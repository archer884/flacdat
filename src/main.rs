use std::{
    borrow::Cow,
    cell::OnceCell,
    collections::HashMap,
    env,
    ffi::OsStr,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    process,
};

use clap::Parser;
use id3::TagLike;
use serde::{Deserialize, Serialize};

type Result<T, E = Error> = std::result::Result<T, E>;

static FFMPEG: &str = "ffmpeg";

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error(transparent)]
    IO(#[from] io::Error),

    #[error(transparent)]
    Id3(#[from] id3::Error),

    #[error(transparent)]
    Vorbis(#[from] metaflac::Error),

    #[error("ffmpeg must be installed")]
    FfmpegNotInstalled,

    #[error("unsupported file type: {0}")]
    UnsupportedFileTye(String),

    #[error(transparent)]
    Csv(#[from] csv::Error),
}

#[derive(Debug, Parser)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Parser)]
enum Command {
    Apply(ApplyAttributes),
    List(List),
    Convert(ConvertToFlac),
}

#[derive(Debug, Parser)]
struct ApplyAttributes {
    /// a file containing attributes to be applied
    ///
    /// By default, attributes will be read from stdin
    #[arg(long)]
    attributes: Option<String>,

    /// directory for output files to be written to
    #[arg(long)]
    output: Option<String>,
}

#[derive(Debug, Parser)]
struct List {
    files: Vec<String>,
}

#[derive(Debug, Parser)]
struct ConvertToFlac {
    files: Vec<String>,
}

impl ConvertToFlac {
    fn wav_paths(&self) -> impl Iterator<Item = impl AsRef<Path> + '_> {
        static EXTENSION: &str = ".wav";
        self.files.iter().filter(|&file| file.ends_with(EXTENSION))
    }
}

#[derive(Debug)]
struct PathGroup<T> {
    base: T,

    // Retained purely as reference material
    mp3_path: OnceCell<PathBuf>,
}

impl<T: AsRef<Path>> PathGroup<T> {
    fn new(base: T) -> Self {
        PathGroup {
            base,
            mp3_path: OnceCell::default(),
        }
    }

    fn flac(&self) -> &Path {
        self.base.as_ref()
    }

    fn flac_output(&self, output_dir: impl AsRef<Path>) -> PathBuf {
        let mut dir = output_dir.as_ref().to_owned();
        dir.push(self.flac().file_name().unwrap());
        dir
    }

    // Retained purely as reference material
    fn mp3(&self) -> &Path {
        self.mp3_path
            .get_or_init(|| self.base.as_ref().with_extension("mp3"))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Attributes {
    album: Option<String>,
    artist: Vec<String>,
    title: Option<String>,
    track: Option<u32>,
    year: Option<i32>,
}

impl Attributes {
    /// Loads attributes for a flac file. Only works on flac files.
    fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        static FLAC: &str = "flac";
        static MP3: &str = "mp3";

        let path = path.as_ref();

        if path.extension() == Some(OsStr::new(FLAC)) {
            return Self::from_flac_path(path);
        }

        if path.extension() == Some(OsStr::new(MP3)) {
            return Self::from_mp3_path(path);
        }

        Err(Error::UnsupportedFileTye(path.display().to_string()))
    }

    fn with_path(self, path: impl AsRef<Path>) -> FileAttributes {
        FileAttributes {
            path: path.as_ref().to_string_lossy().into(),
            album: self.album,
            artist: self.artist,
            title: self.title,
            track: self.track,
            year: self.year,
        }
    }

    fn from_flac_path(path: &Path) -> Result<Self> {
        let mut flac = metaflac::Tag::read_from_path(path)?;
        let comment = flac.vorbis_comments_mut();

        Ok(Attributes {
            album: comment
                .album()
                .into_iter()
                .flatten()
                .next()
                .map(|s| s.into()),
            artist: comment.artist().cloned().unwrap_or_default(),
            title: comment
                .title()
                .into_iter()
                .flatten()
                .next()
                .map(|s| s.into()),
            track: comment.track().into_iter().next(),

            // This is basically unfindable on a flac/vorbis file:
            // https://www.reddit.com/r/musichoarder/comments/p20pzi/how_do_you_store_date_tags_in_flacvorbis_comment/
            year: comment
                .get("YEAR")
                .into_iter()
                .flatten()
                .next()
                .and_then(|s| s.parse().ok()),
        })
    }

    fn from_mp3_path(path: &Path) -> Result<Self> {
        let tag = id3::Tag::read_from_path(path)?;

        Ok(Attributes {
            album: tag.album().map(|s| s.to_string()),
            artist: tag
                .artist()
                .map(|s| vec![s.to_string()])
                .unwrap_or_default(),
            title: tag.title().map(|s| s.to_string()),
            track: tag.track(),
            year: tag.year(),
        })
    }
}

enum Attribute {
    Album,
    Artist,
    Title,
    Track,
    Year,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct FileAttributes {
    path: String,
    album: Option<String>,
    artist: Vec<String>,
    title: Option<String>,
    track: Option<u32>,
    year: Option<i32>,
}

fn main() {
    if let Err(e) = run(Args::parse_from(wild::args())) {
        eprintln!("{e}");
        process::exit(1);
    }
}

fn run(args: Args) -> Result<()> {
    if let Some(command) = &args.command {
        return dispatch(command);
    }

    Ok(())
}

fn dispatch(command: &Command) -> Result<()> {
    match command {
        Command::Apply(args) => apply_attributes(args),
        Command::List(args) => list_attributes(args),
        Command::Convert(convert_args) => convert_wav_to_flac(convert_args),
    }
}

fn apply_attributes(args: &ApplyAttributes) -> Result<()> {
    let output: Cow<_> = match args.output.as_ref() {
        Some(output) => Path::new(output).into(),
        None => env::current_dir()?.into(),
    };

    if !output.exists() {
        fs::create_dir(&output)?;
    }

    let attributes = read_attributes(args)?;

    for (path, attr) in attributes {
        let paths = PathGroup::new(&path);
        let mut flac = metaflac::Tag::read_from_path(&path)?;
        let comment = flac.vorbis_comments_mut();

        if let Some(album) = attr.album {
            comment.set_album(vec![album.to_string()]);
        }
        if let Some(title) = attr.title {
            comment.set_title(vec![title]);
        }
        if let Some(track) = attr.track {
            comment.set_track(track);
        }
        comment.set_artist(attr.artist);

        let output_name = paths.flac_output(&output);
        if output_name.exists() {
            return Err(Error::IO(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "writing metadata would overwrite existing file",
            )));
        }
        fs::copy(paths.flac(), &output_name)?;
        flac.write_to_path(&output_name)?;
    }

    Ok(())
}

fn list_attributes(args: &List) -> Result<()> {
    let collection: Result<Vec<_>> = args
        .files
        .iter()
        .map(|path| Attributes::from_path(path).map(|attributes| attributes.with_path(path)))
        .collect();
    let collection = collection?;

    let mut out = io::stdout().lock();
    let mut writer = csv::WriterBuilder::new()
        .delimiter(b'\t')
        .from_writer(&mut out);
    writer.write_record(&["path", "album", "artist", "title", "track", "year"])?;

    for item in collection {
        writer.write_field(&item.path)?;

        if let Some(album) = &item.album {
            writer.write_field(&album)?;
        } else {
            writer.write_field("")?;
        }

        writer.write_field(item.artist.join(","))?;

        if let Some(title) = &item.title {
            writer.write_field(&title)?;
        } else {
            writer.write_field("")?;
        }

        if let Some(track) = item.track {
            writer.write_field(track.to_string())?;
        } else {
            writer.write_field("")?;
        }

        if let Some(year) = item.year {
            writer.write_field(year.to_string())?;
        } else {
            writer.write_field("")?;
        }

        writer.write_record(None::<&[u8]>)?;
    }

    writer.flush()?;

    Ok(())
}

fn convert_wav_to_flac(args: &ConvertToFlac) -> Result<()> {
    ensure_ffmpeg()?;

    assert!(args.wav_paths().next().is_some());

    for path in args.wav_paths() {
        let path = dbg!(path.as_ref());
        let flac_path = dbg!(path.with_extension("flac"));

        process::Command::new(FFMPEG)
            .arg("-i")
            .arg(path)
            .arg(flac_path)
            .status()?;
    }

    Ok(())
}

fn ensure_ffmpeg() -> Result<()> {
    process::Command::new(FFMPEG)
        .output()
        .map_err(|_| Error::FfmpegNotInstalled)?;
    Ok(())
}

fn read_attributes(args: &ApplyAttributes) -> Result<HashMap<String, FileAttributes>> {
    let text = match &args.attributes {
        Some(path) => fs::read_to_string(path)?,
        None => {
            let mut buf = String::new();
            io::stdin().lock().read_to_string(&mut buf)?;
            buf
        }
    };

    let mut bytes = text.as_bytes();
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(b'\t')
        .from_reader(&mut bytes);
    let attributes: csv::Result<Vec<FileAttributes>> = reader.deserialize().collect();
    let attributes = attributes?;

    Ok(attributes
        .into_iter()
        .map(|attributes| (attributes.path.clone(), attributes))
        .collect())
}
