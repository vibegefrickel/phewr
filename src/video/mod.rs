//! The lookup of the `ffmpeg` and `ffprobe` programs, and the first frame and the grid loop
//! with the programs of the session.
//!
//! phewr links no `FFmpeg` library. It starts the programs when PATH has them. Without them, a
//! video cell keeps its play icon, and a video opens in the mpv window. The decode code is in
//! `tuilibr::pictures::motion`.

use image::RgbaImage;
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;
use tuilibr::pictures::motion::first_frame::first_frame_of;
use tuilibr::pictures::motion::loop_frames::{LoopFrames, loop_of};
use tuilibr::programs::find_on_path;

/// `None` when PATH has no such program. `file_meta` runs ffprobe without ffmpeg.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Tools {
    pub(crate) ffmpeg: Option<PathBuf>,
    pub(crate) ffprobe: Option<PathBuf>,
}

impl Tools {
    /// Find both programs in the folders of the PATH value `path`.
    pub(crate) fn find(path: Option<&OsStr>) -> Self {
        Self {
            ffmpeg: find_on_path("ffmpeg", path),
            ffprobe: find_on_path("ffprobe", path),
        }
    }

    /// Without one of the programs, the error is `NotFound` and no process starts.
    fn motion(&self) -> io::Result<tuilibr::pictures::motion::Tools> {
        let (Some(ffmpeg), Some(ffprobe)) = (&self.ffmpeg, &self.ffprobe) else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "ffmpeg or ffprobe not found on PATH",
            ));
        };
        Ok(tuilibr::pictures::motion::Tools {
            ffmpeg: ffmpeg.clone(),
            ffprobe: ffprobe.clone(),
        })
    }
}

/// Only [`find_session_tools`] fills it.
static SESSION_TOOLS: OnceLock<Tools> = OnceLock::new();

static NO_TOOLS: Tools = Tools {
    ffmpeg: None,
    ffprobe: None,
};

/// Look up `ffmpeg` and `ffprobe` once at the start. `main` calls it, and the tests never do.
pub(crate) fn find_session_tools() {
    SESSION_TOOLS.get_or_init(|| Tools::find(std::env::var_os("PATH").as_deref()));
}

/// No programs before the lookup.
pub(crate) fn session_tools() -> &'static Tools {
    SESSION_TOOLS.get().unwrap_or(&NO_TOOLS)
}

/// The first frame of `path` for a grid thumbnail, with the programs of the session.
pub(crate) fn session_first_frame(
    path: &Path,
    target: (u32, u32),
    cache_dir: &Path,
) -> io::Result<RgbaImage> {
    first_frame_of(&session_tools().motion()?, path, target, cache_dir)
}

/// The grid loop of `path`, with the programs of the session.
pub(crate) fn session_loop(
    path: &Path,
    target: (u32, u32),
    cache_dir: &Path,
    cancel: &AtomicBool,
) -> io::Result<LoopFrames> {
    loop_of(&session_tools().motion()?, path, target, cache_dir, cancel)
}

#[cfg(test)]
mod tests {
    use super::{Tools, session_first_frame};
    use std::ffi::OsString;
    use std::path::Path;
    use tuilibr::pictures::motion::first_frame::first_frame_of;

    #[test]
    fn lookup_finds_tools_on_path() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["ffmpeg", "ffprobe"] {
            let program = dir.path().join(name);
            std::fs::write(&program, "#!/bin/sh\n").unwrap();
            let mut permissions = std::fs::metadata(&program).unwrap().permissions();
            std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
            std::fs::set_permissions(&program, permissions).unwrap();
        }
        let path = OsString::from(dir.path());
        let tools = Tools::find(Some(&path));
        assert_eq!(tools.ffmpeg, Some(dir.path().join("ffmpeg")));
        assert_eq!(tools.ffprobe, Some(dir.path().join("ffprobe")));
        assert_eq!(Tools::find(None), Tools::default());
    }

    #[test]
    fn no_session_lookup_starts_nothing() {
        let error = session_first_frame(Path::new("clip.mp4"), (32, 24), Path::new("/dev/null"))
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }

    /// Fake paths that hold no program: the probe fails to start, and no ffmpeg runs.
    #[test]
    fn tools_reach_the_decoder() {
        let dir = tempfile::tempdir().unwrap();
        let tools = Tools {
            ffmpeg: Some(dir.path().join("ffmpeg")),
            ffprobe: Some(dir.path().join("ffprobe")),
        };
        let motion = tools.motion().unwrap();
        assert_eq!(motion.ffmpeg, dir.path().join("ffmpeg"));
        assert_eq!(motion.ffprobe, dir.path().join("ffprobe"));
        let missing = Tools {
            ffprobe: None,
            ..tools
        };
        let missing_error = missing.motion().unwrap_err();
        assert_eq!(missing_error.kind(), std::io::ErrorKind::NotFound);

        let clip = dir.path().join("clip.mp4");
        std::fs::write(&clip, b"no video").unwrap();
        let error = first_frame_of(&motion, &clip, (32, 24), dir.path()).unwrap_err();
        assert_ne!(
            error.to_string(),
            missing_error.to_string(),
            "the tools pass the lookup"
        );
    }
}
