use std::{
    cell::OnceCell,
    fs, io,
    path::{Path, PathBuf},
    process,
};

use clap::Parser;
use metaflac::Tag;

type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error(transparent)]
    IO(#[from] io::Error),

    #[error(transparent)]
    Id3(#[from] audiotags::Error),

    #[error(transparent)]
    Vorbis(#[from] metaflac::Error),
}

#[derive(Debug, Parser)]
struct Args {
    dir: String,
}

#[derive(Debug)]
struct PathGroup<T> {
    base: T,
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
        let mut dir = PathBuf::from(output_dir.as_ref().to_owned());
        dir.push(self.flac().file_name().unwrap());
        dir
    }

    fn mp3(&self) -> &Path {
        self.mp3_path
            .get_or_init(|| self.base.as_ref().with_extension("mp3"))
    }

    fn track(&self) -> Option<usize> {
        let s = self.base.as_ref().file_name()?.to_str()?;
        let (track, _) = s.split_once(' ')?;
        track.parse().ok()
    }
}

fn main() {
    if let Err(e) = run(Args::parse()) {
        eprintln!("{e}");
        process::exit(1);
    }
}

fn run(args: Args) -> Result<()> {
    let paths = read_flac_paths(&args.dir)?;
    let output_dir = build_output_directory(&args.dir)?;

    for path in &paths {
        let path_group = PathGroup::new(path);
        let tag = audiotags::Tag::new().read_from_path(path_group.mp3())?;
        let mut flac = Tag::read_from_path(path_group.flac())?;
        let comment = flac.vorbis_comments_mut();

        if let Some(album) = tag.album().map(|album| album.title) {
            comment.set_album(vec![album]);
        }

        if let Some(title) = tag.title() {
            comment.set_title(vec![title]);
        }

        if let Some(artist) = tag.artist() {
            comment.set_artist(vec![artist]);
        }

        if let Some(track) = path_group.track() {
            comment.set_track(track as u32);
        }

        let output_name = path_group.flac_output(&output_dir);
        fs::copy(path_group.flac(), path_group.flac_output(&output_dir))?;
        flac.write_to_path(&output_name)?;
    }

    Ok(())
}

fn read_flac_paths(path: &str) -> io::Result<Vec<PathBuf>> {
    Ok(fs::read_dir(path)?
        .into_iter()
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let meta = entry.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }

            let path = entry.path();
            Some(path).filter(|path| {
                path.extension()
                    .map(|ext| ext == &*"flac")
                    .unwrap_or_default()
            })
        })
        .collect())
}

fn build_output_directory(path: &str) -> io::Result<PathBuf> {
    let mut path = PathBuf::from(path);
    path.push("with_metadata");

    if path.exists() {
        if fs::read_dir(&path)?.next().is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "the output directory already exists and is non-empty",
            ));
        }
    } else {
        fs::create_dir(&path)?;
    }

    return Ok(path);
}
