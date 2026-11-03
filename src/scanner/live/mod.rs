//! The pairs of a Live Photo: a still (HEIC or JPEG) and its motion, a `.mov` with the same
//! base name.
//!
//! The scan pairs by the base name alone, on the sorted files, before the first batch. The
//! motion then has no cell, and the cell list stays stable from the first batch on. The content
//! identifiers wait for the start of the motion (`identifier.rs`): a read of every pair costs
//! 0.2 s to 2.3 s for a folder of 10 000 files (`docs/live-photos.md`).

#[cfg(feature = "video")]
mod identifier;

#[cfg(feature = "video")]
pub(crate) use identifier::identifiers_differ;

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The camera writes HEIC, or JPEG with "Most Compatible".
const STILL_EXTENSIONS: &[&str] = &["heic", "heif", "jpg", "jpeg"];
/// A HEIF still wins over its JPEG twin.
const HEIF_STILL_EXTENSIONS: &[&str] = &["heic", "heif"];
const MOTION_SUFFIX: &[u8] = b".mov";
/// A still sorts before its motion (`heic` and `jpg` before `mov`). Only a name with the same
/// stem and an extension between them sorts in between, a JPEG twin for example.
const LOOK_BACK: usize = 3;

/// The motion of each Live Photo still of one folder, keyed by the still.
///
/// The key is the plain string of the path: the hash of a `Path` walks its components, and the
/// scan and the lookups use the same paths.
#[derive(Debug, Default)]
pub(crate) struct LivePhotos {
    motions: HashMap<OsString, PathBuf>,
}

impl LivePhotos {
    /// The motion of `still`, or `None` for a file that is no Live Photo still.
    pub(crate) fn motion(&self, still: &Path) -> Option<&Path> {
        self.motions.get(still.as_os_str()).map(PathBuf::as_path)
    }

    /// The still of a hidden motion. The search goes through all pairs, and only a start with a
    /// cursor target runs it.
    pub(crate) fn still_of(&self, motion: &Path) -> Option<&Path> {
        self.motions
            .iter()
            .find(|(_, candidate)| candidate.as_path() == motion)
            .map(|(still, _)| Path::new(still))
    }

    /// The pairs `(still, motion)` without a scan.
    #[cfg(test)]
    pub(crate) fn from_pairs(pairs: impl IntoIterator<Item = (PathBuf, PathBuf)>) -> Self {
        Self {
            motions: pairs
                .into_iter()
                .map(|(still, motion)| (still.into_os_string(), motion))
                .collect(),
        }
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.motions.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.motions.len()
    }
}

/// Take the motion of each Live Photo still out of `files`, which follow `name_key`.
///
/// The pass looks at the `LOOK_BACK` names before each `.mov` and reads no file. A still pairs
/// with one motion only.
pub(crate) fn pair(files: &mut Vec<PathBuf>) -> LivePhotos {
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    for motion in 0..files.len() {
        if !is_motion(&files[motion]) {
            continue;
        }
        let paired = &pairs[pairs.len().saturating_sub(LOOK_BACK)..];
        if let Some(still) = still_for(files, motion, paired) {
            pairs.push((still, motion));
        }
    }
    if pairs.is_empty() {
        return LivePhotos::default();
    }
    let mut motions = HashMap::with_capacity(pairs.len());
    for &(still, motion) in &pairs {
        // The empty path marks the motion for the removal below. A listed path is never empty.
        let motion_path = std::mem::take(&mut files[motion]);
        motions.insert(files[still].as_os_str().to_owned(), motion_path);
    }
    files.retain(|path| !path.as_os_str().is_empty());
    LivePhotos { motions }
}

/// The still of the motion at `motion`: the nearest HEIF still with the same stem, else the
/// nearest JPEG still. `recent` holds the last pairs: the pairs ascend, so they hold each paired
/// still in the window.
fn still_for(files: &[PathBuf], motion: usize, recent: &[(usize, usize)]) -> Option<usize> {
    let name = files[motion].as_os_str().as_encoded_bytes();
    let base = &name[..name.len() - MOTION_SUFFIX.len()];
    (motion.saturating_sub(LOOK_BACK)..motion)
        .rev()
        .filter(|&index| recent.iter().all(|&(still, _)| still != index))
        .filter_map(|index| Some((index, still_kind(&files[index], base)?)))
        .min_by_key(|&(_, heif)| !heif)
        .map(|(index, _)| index)
}

/// `Some(true)` for the HEIF still `{base}.heic`, `Some(false)` for a JPEG still, else `None`.
/// The bytes compare without regard to ASCII case, like the lowercase sort key. The pass parses
/// no path: the files of one folder share the folder part.
fn still_kind(path: &Path, base: &[u8]) -> Option<bool> {
    let bytes = path.as_os_str().as_encoded_bytes();
    let (head, extension) = bytes.split_at_checked(base.len())?;
    let extension = extension.strip_prefix(b".")?;
    if !head.eq_ignore_ascii_case(base) {
        return None;
    }
    let known = |list: &[&str]| {
        list.iter()
            .any(|known| extension.eq_ignore_ascii_case(known.as_bytes()))
    };
    known(STILL_EXTENSIONS).then(|| known(HEIF_STILL_EXTENSIONS))
}

/// The scan pass calls it for every file. The test of the last bytes parses no extension.
fn is_motion(path: &Path) -> bool {
    let name = path.as_os_str().as_encoded_bytes();
    name.len() > MOTION_SUFFIX.len()
        && name[name.len() - MOTION_SUFFIX.len()..].eq_ignore_ascii_case(MOTION_SUFFIX)
}

#[cfg(test)]
mod tests;
