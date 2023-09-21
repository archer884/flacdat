use std::{
    cell::OnceCell,
    fs, io,
    path::{Path, PathBuf},
    process,
};

// use audiotags::{AudioTagEdit, AudioTagWrite, FlacTag, Tag};
use clap::Parser;
use metaflac::{block::VorbisComment, Block, Tag};

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

    // for testing
    fn flac_with_name(&self, name: &str) -> PathBuf {
        self.base.as_ref().with_file_name(name)
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
    for path in &paths {
        // For each path, we will find (presumably) a flac file. There will be a corresponding mp3
        // file, which should hypothetically contain metadata. We'll start with getting that
        // metadata and printing it.

        let path_group = PathGroup::new(path);
        let tag = audiotags::Tag::new().read_from_path(path_group.mp3())?;

        // When we have the metadata, apparently we also have a writer for said metadata, which we
        // can then use to spray and pray. Who knew?

        // Update: That shit doesn't even kind of work. I'm trying to do it manually now.

        let mut flac = Tag::read_from_path(path_group.flac())?;
        let comment = flac.vorbis_comments_mut();

        if let Some(album) = tag.album() {
            comment.set_album(vec![album.title]);
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

        flac.write_to_path(path_group.flac())?;
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
