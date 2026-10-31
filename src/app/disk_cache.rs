//! The thumbnail disk cache: the key, the read, the write and the prune. It also keeps the
//! frames of the grid loops, one file per loop.

use super::grid_loops::LoopFrames;
use directories::ProjectDirs;
use image::RgbaImage;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// The version in the disk cache key. Change it when the thumbnail output changes.
const THUMBNAIL_CACHE_VERSION: &str = "viewr-thumbnail-v13";

/// The version in the key of a loop file. Change it when the loop output changes.
const LOOP_CACHE_VERSION: &str = "viewr-loop-v1";

/// The extension of a loop file: the frames of one grid loop as QOI images, with their times.
const LOOP_EXTENSION: &str = "loop";

/// The first bytes of a loop file.
const LOOP_MAGIC: &[u8; 8] = b"VIEWRLP1";

/// The file format of the cache. QOI decodes twice as fast as PNG at the same file size.
const CACHE_FORMAT: image::ImageFormat = image::ImageFormat::Qoi;

/// The extension of a cache file.
const CACHE_EXTENSION: &str = "qoi";

/// The extension of the PNG files of the older versions. Their keys never hit again, so the
/// prune removes them.
const OLD_CACHE_EXTENSION: &str = "png";

/// The thumbnail disk cache below the user's cache directory, or a temp directory without one.
pub(super) fn default_disk_cache_dir() -> PathBuf {
    ProjectDirs::from("", "", "viewr").map_or_else(
        || std::env::temp_dir().join("viewr-cache"),
        |dirs| dirs.cache_dir().to_path_buf(),
    )
}

/// The identity of a source file in the cache key: its length and its modification time. A
/// changed file gets another stamp, so its old cache file and its old animation flag never hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FileStamp {
    len: u64,
    mtime: Duration,
}

impl FileStamp {
    /// The stamp of `metadata`, or `None` when its modification time does not read.
    fn of(metadata: &std::fs::Metadata) -> Option<Self> {
        let mtime = metadata
            .modified()
            .ok()?
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .ok()?;
        Some(Self {
            len: metadata.len(),
            mtime,
        })
    }
}

/// The cache file of one source file at one size, with the stamp that its name hashes. A
/// thumbnail job computes it one time and reads and writes through it, because the key costs
/// a `stat` and a `realpath` walk.
#[derive(Debug)]
pub(super) struct CacheEntry {
    file: PathBuf,
    #[cfg_attr(
        not(feature = "video"),
        expect(
            dead_code,
            reason = "only the animation flag of the video feature reads it"
        )
    )]
    pub(super) stamp: FileStamp,
}

impl CacheEntry {
    /// The cached thumbnail. A file that does not decode is removed.
    pub(super) fn load(&self) -> Option<RgbaImage> {
        let bytes = std::fs::read(&self.file).ok()?;
        if let Ok(image) = image::load_from_memory_with_format(&bytes, CACHE_FORMAT) {
            Some(image.into_rgba8())
        } else {
            #[expect(
                clippy::let_underscore_must_use,
                reason = "a cache file that stays is only disk space, the cache prunes it"
            )]
            let _ = std::fs::remove_file(&self.file);
            None
        }
    }

    /// Keep `img` as the thumbnail of this entry.
    pub(super) fn save(&self, img: &RgbaImage) -> io::Result<()> {
        write_cache_file(&self.file, |temp_path| {
            img.save_with_format(temp_path, CACHE_FORMAT)
                .map_err(io::Error::other)
        })
    }

    /// True when the cache holds this thumbnail, so the folder prefetch decodes nothing.
    pub(super) fn exists(&self) -> bool {
        self.file.exists()
    }
}

/// The cache entry of the thumbnail of `path` at the pixel size `target`. The name hashes the
/// file identity, so a changed file or another size never hits an old thumbnail.
pub(super) fn thumbnail_entry(
    cache_dir: &Path,
    path: &Path,
    target: (u32, u32),
) -> Option<CacheEntry> {
    cache_entry(
        cache_dir,
        path,
        target,
        THUMBNAIL_CACHE_VERSION,
        CACHE_EXTENSION,
    )
}

/// The cache entry of one thumbnail job, computed at the first use. A job that reads no cache,
/// saves nothing and needs no file stamp computes no key, for example the HEIC preview of a
/// cell whose cache read missed.
pub(super) struct LazyEntry<'a> {
    cache_dir: &'a Path,
    path: &'a Path,
    target: (u32, u32),
    entry: std::cell::OnceCell<Option<CacheEntry>>,
}

impl<'a> LazyEntry<'a> {
    pub(super) fn new(cache_dir: &'a Path, path: &'a Path, target: (u32, u32)) -> Self {
        Self {
            cache_dir,
            path,
            target,
            entry: std::cell::OnceCell::new(),
        }
    }

    /// The entry, computed at the first call. `None` for a file without a key.
    pub(super) fn get(&self) -> Option<&CacheEntry> {
        self.entry
            .get_or_init(|| thumbnail_entry(self.cache_dir, self.path, self.target))
            .as_ref()
    }

    /// The cached thumbnail, or `None` for a miss or a file without a key.
    pub(super) fn load(&self) -> Option<RgbaImage> {
        self.get()?.load()
    }

    /// Keep `img` in the cache. A file without a key stays out of the cache.
    pub(super) fn save(&self, img: &RgbaImage) -> io::Result<()> {
        self.get().ok_or_else(no_cache_key)?.save(img)
    }
}

/// The cache file of the thumbnail of `path` at `target`, for the tests that look for it.
#[cfg(test)]
pub(super) fn get_cache_file_path(
    cache_dir: &Path,
    path: &Path,
    target: (u32, u32),
) -> Option<PathBuf> {
    thumbnail_entry(cache_dir, path, target).map(|entry| entry.file)
}

/// The cache entry of `path` at `target` for the output `version`, with `extension`.
fn cache_entry(
    cache_dir: &Path,
    path: &Path,
    target: (u32, u32),
    version: &str,
    extension: &str,
) -> Option<CacheEntry> {
    use std::collections::hash_map::DefaultHasher;
    use std::fs;
    use std::hash::{Hash, Hasher};

    #[cfg(test)]
    KEY_COMPUTATIONS.add(path);
    let stamp = FileStamp::of(&fs::metadata(path).ok()?)?;
    let canonical_path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

    let mut hasher = DefaultHasher::new();
    version.hash(&mut hasher);
    crate::decode::color::target().hash(&mut hasher);
    canonical_path.hash(&mut hasher);
    stamp.len.hash(&mut hasher);
    stamp.mtime.as_secs().hash(&mut hasher);
    stamp.mtime.subsec_nanos().hash(&mut hasher);
    target.0.hash(&mut hasher);
    target.1.hash(&mut hasher);
    let filename = format!("{:016x}.{extension}", hasher.finish());

    Some(CacheEntry {
        file: cache_dir.join(filename),
        stamp,
    })
}

/// The cache file of `path` at `target` for the output `version`, with `extension`.
fn cache_file_path(
    cache_dir: &Path,
    path: &Path,
    target: (u32, u32),
    version: &str,
    extension: &str,
) -> Option<PathBuf> {
    cache_entry(cache_dir, path, target, version, extension).map(|entry| entry.file)
}

/// The cached thumbnail of `path` at `target`, for the tests that read the cache directly.
#[cfg(test)]
pub(super) fn try_load_from_disk_cache(
    cache_dir: &Path,
    path: &Path,
    target: (u32, u32),
) -> Option<RgbaImage> {
    thumbnail_entry(cache_dir, path, target)?.load()
}

/// Keep `img` as the thumbnail of `path` at `target`, for the tests that fill the cache.
#[cfg(test)]
pub(super) fn try_save_to_disk_cache(
    cache_dir: &Path,
    path: &Path,
    target: (u32, u32),
    img: &RgbaImage,
) -> io::Result<()> {
    thumbnail_entry(cache_dir, path, target)
        .ok_or_else(no_cache_key)?
        .save(img)
}

/// The calls per source path of one function, so that a test counts the file reads of a job.
/// Each test uses its own temp paths, so the tests that run at the same time do not mix. A
/// `BTreeMap` has a `const` constructor, and one entry per path keeps the cost fixed when all
/// tests share one process (`cargo test`).
#[cfg(test)]
pub(super) struct PathCounter(std::sync::Mutex<std::collections::BTreeMap<PathBuf, usize>>);

#[cfg(test)]
impl PathCounter {
    pub(super) const fn new() -> Self {
        Self(std::sync::Mutex::new(std::collections::BTreeMap::new()))
    }

    /// Count one call for `path`.
    pub(super) fn add(&self, path: &Path) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(path.to_path_buf())
            .or_default() += 1;
    }

    /// The calls for `path` so far.
    pub(super) fn of(&self, path: &Path) -> usize {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(path)
            .copied()
            .unwrap_or_default()
    }
}

/// The cache keys that `cache_entry` computed, per source path.
#[cfg(test)]
pub(super) static KEY_COMPUTATIONS: PathCounter = PathCounter::new();

/// The error of a file without a cache key: the file is gone, or its time does not read.
fn no_cache_key() -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, "the file has no cache key")
}

/// The frames of the grid loop of `path` for a grid picture of `target` pixels, from the disk
/// cache. A file that does not read is removed.
pub(super) fn try_load_loop(
    cache_dir: &Path,
    path: &Path,
    target: (u32, u32),
) -> Option<LoopFrames> {
    let cache_path = cache_file_path(cache_dir, path, target, LOOP_CACHE_VERSION, LOOP_EXTENSION)?;
    let bytes = std::fs::read(&cache_path).ok()?;
    let frames = decode_loop_file(&bytes);
    if frames.is_none() {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "a cache file that stays is only disk space, the cache prunes it"
        )]
        let _ = std::fs::remove_file(cache_path);
    }
    frames
}

/// Keep the frames of the grid loop of `path` in the disk cache, so that a warm folder starts
/// its loops without ffmpeg.
pub(super) fn try_save_loop(
    cache_dir: &Path,
    path: &Path,
    target: (u32, u32),
    frames: &LoopFrames,
) -> io::Result<()> {
    let cache_path = cache_file_path(cache_dir, path, target, LOOP_CACHE_VERSION, LOOP_EXTENSION)
        .ok_or_else(no_cache_key)?;
    let bytes = encode_loop_file(frames).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "the loop does not fit the loop file",
        )
    })?;
    write_cache_file(&cache_path, |temp_path| std::fs::write(temp_path, &bytes))
}

/// A loop file: [`LOOP_MAGIC`], the frame count and the size of the still, then the time in
/// microseconds, the length and the QOI bytes of each frame. All numbers are little-endian
/// `u32` values.
fn encode_loop_file(frames: &LoopFrames) -> Option<Vec<u8>> {
    let mut bytes = LOOP_MAGIC.to_vec();
    let count = u32::try_from(frames.frames.len()).ok()?;
    for number in [count, frames.shown.0, frames.shown.1] {
        bytes.extend_from_slice(&number.to_le_bytes());
    }
    for (frame, delay) in frames.frames.iter().zip(&frames.delays) {
        let mut qoi = Vec::new();
        frame
            .write_to(&mut io::Cursor::new(&mut qoi), CACHE_FORMAT)
            .ok()?;
        let micros = u32::try_from(delay.as_micros()).ok()?;
        let length = u32::try_from(qoi.len()).ok()?;
        bytes.extend_from_slice(&micros.to_le_bytes());
        bytes.extend_from_slice(&length.to_le_bytes());
        bytes.extend_from_slice(&qoi);
    }
    Some(bytes)
}

/// The frames of a loop file, or `None` for a file that is cut or broken.
fn decode_loop_file(bytes: &[u8]) -> Option<LoopFrames> {
    let mut rest = bytes.strip_prefix(LOOP_MAGIC.as_slice())?;
    let count = take_u32(&mut rest)?;
    // A loop of fewer than 2 frames never plays, and `CellLoop::id_at` needs 2.
    if count < 2 {
        return None;
    }
    let shown = (take_u32(&mut rest)?, take_u32(&mut rest)?);
    let mut frames = LoopFrames {
        frames: Vec::new(),
        delays: Vec::new(),
        shown,
    };
    for _ in 0..count {
        let micros = take_u32(&mut rest)?;
        let length = usize::try_from(take_u32(&mut rest)?).ok()?;
        let (qoi, tail) = rest.split_at_checked(length)?;
        rest = tail;
        let image = image::load_from_memory_with_format(qoi, CACHE_FORMAT).ok()?;
        frames.frames.push(image.into_rgba8());
        frames.delays.push(Duration::from_micros(u64::from(micros)));
    }
    Some(frames)
}

/// The little-endian `u32` at the start of `rest`, which then starts after it.
fn take_u32(rest: &mut &[u8]) -> Option<u32> {
    let (number, tail) = rest.split_first_chunk::<4>()?;
    *rest = tail;
    Some(u32::from_le_bytes(*number))
}

/// Write a cache file through a temp file and a rename, so that a reader never sees half a
/// file. `write` fills the temp file. The first write of the session prunes the cache, and
/// each 64th write too.
fn write_cache_file(
    cache_path: &Path,
    write: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<()> {
    static CACHE_PRUNED: OnceLock<()> = OnceLock::new();
    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    write_cache_file_with(cache_path, write, &CACHE_PRUNED, &TEMP_SEQUENCE, &|dir| {
        prune_disk_cache(dir, 512 * 1024 * 1024);
    })
}

/// [`write_cache_file`] with its session state and its prune as arguments, so that a test
/// counts the prune runs.
fn write_cache_file_with(
    cache_path: &Path,
    write: impl FnOnce(&Path) -> io::Result<()>,
    pruned: &OnceLock<()>,
    temp_sequence: &AtomicU64,
    prune: &dyn Fn(&Path),
) -> io::Result<()> {
    use std::fs;

    let no_name = || io::Error::new(io::ErrorKind::InvalidInput, "the cache file has no name");
    let parent = cache_path.parent().ok_or_else(no_name)?;
    fs::create_dir_all(parent)?;
    pruned.get_or_init(|| prune(parent));
    if cache_path.exists() {
        return Ok(());
    }

    let sequence = temp_sequence.fetch_add(1, Ordering::Relaxed);
    let cache_name = cache_path
        .file_name()
        .ok_or_else(no_name)?
        .to_string_lossy();
    let temp_path = parent.join(format!(
        ".{cache_name}.{}.{}.tmp",
        std::process::id(),
        sequence
    ));
    if let Err(error) = write(&temp_path) {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "a cache file that stays is only disk space, the cache prunes it"
        )]
        let _ = fs::remove_file(temp_path);
        return Err(error);
    }
    if let Err(error) = fs::rename(&temp_path, cache_path) {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "a cache file that stays is only disk space, the cache prunes it"
        )]
        let _ = fs::remove_file(temp_path);
        if !cache_path.exists() {
            return Err(error);
        }
    }
    if prunes_after_write(sequence) {
        prune(parent);
    }
    Ok(())
}

/// Whether the write with this sequence number prunes. Write 0 skips it, because the first
/// write of the session pruned the cache a moment before.
fn prunes_after_write(sequence: u64) -> bool {
    sequence != 0 && sequence.is_multiple_of(64)
}

/// The age after which a temp file of the cache counts as left behind. A write takes
/// milliseconds, so an older temp file belongs to a viewr that ended during its write.
const STALE_TEMP_AGE: Duration = Duration::from_secs(3600);

/// Remove a temp file of the cache or a partial cube file that is older than `STALE_TEMP_AGE`. A quit
/// during a cache write leaves one, and the size prune only counts the cache files.
fn remove_stale_temp_file(entry: &std::fs::DirEntry) {
    let path = entry.path();
    let extension = path.extension().and_then(|ext| ext.to_str());
    // The thumbnail temp files start with a dot, the partial cube files of the tone map end in
    // `.partial` (`player/tone_map.rs`).
    let is_temp = (extension == Some("tmp")
        && entry.file_name().to_string_lossy().starts_with('.'))
        || extension == Some("partial");
    let is_stale = entry
        .metadata()
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age > STALE_TEMP_AGE);
    if is_temp && is_stale {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "a temp file that stays is only disk space, the next prune tries again"
        )]
        let _ = std::fs::remove_file(path);
    }
}

/// Remove cache files until the cache takes at most `max_bytes`: the loop files first, then
/// the stills, each oldest first. A video loop takes about 3 times the bytes of its still and
/// an animation up to 12 times, so the stills of a warm folder keep their priority.
fn prune_disk_cache(cache_dir: &Path, max_bytes: u64) {
    let Ok(entries) = std::fs::read_dir(cache_dir) else {
        return;
    };
    let mut files: Vec<(PathBuf, u64, std::time::SystemTime)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let extension = path.extension().and_then(|ext| ext.to_str());
            if extension == Some(OLD_CACHE_EXTENSION) {
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "an old file that stays is only disk space, the next prune tries again"
                )]
                let _ = std::fs::remove_file(&path);
                return None;
            }
            if extension != Some(CACHE_EXTENSION) && extension != Some(LOOP_EXTENSION) {
                remove_stale_temp_file(&entry);
                return None;
            }
            let metadata = entry.metadata().ok()?;
            metadata.is_file().then(|| {
                (
                    path,
                    metadata.len(),
                    metadata.modified().unwrap_or(std::time::UNIX_EPOCH),
                )
            })
        })
        .collect();
    let mut total_bytes: u64 = files.iter().map(|(_, bytes, _)| bytes).sum();
    if total_bytes <= max_bytes {
        return;
    }

    files.sort_by_key(|(path, _, modified)| {
        let still = path.extension().and_then(|ext| ext.to_str()) == Some(CACHE_EXTENSION);
        (still, *modified)
    });
    for (path, bytes, _) in files {
        if total_bytes <= max_bytes {
            break;
        }
        if std::fs::remove_file(path).is_ok() {
            total_bytes = total_bytes.saturating_sub(bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LoopFrames, decode_loop_file, get_cache_file_path, prune_disk_cache, prunes_after_write,
        try_load_from_disk_cache, try_load_loop, try_save_loop, try_save_to_disk_cache,
        write_cache_file_with,
    };
    use crate::test_support::test_dir;
    use image::RgbaImage;
    use std::fs;

    use std::time::{Duration, SystemTime};

    #[test]
    fn a_loop_file_of_fewer_than_two_frames_reads_as_a_miss() {
        for count in [0_u32, 1] {
            let mut bytes = super::LOOP_MAGIC.to_vec();
            for number in [count, 8, 4] {
                bytes.extend_from_slice(&number.to_le_bytes());
            }
            if count == 1 {
                let mut qoi = Vec::new();
                RgbaImage::from_pixel(4, 2, image::Rgba([1, 2, 3, 255]))
                    .write_to(&mut std::io::Cursor::new(&mut qoi), super::CACHE_FORMAT)
                    .unwrap();
                bytes.extend_from_slice(&1000_u32.to_le_bytes());
                bytes.extend_from_slice(&u32::try_from(qoi.len()).unwrap().to_le_bytes());
                bytes.extend_from_slice(&qoi);
            }
            assert!(decode_loop_file(&bytes).is_none(), "count {count}");
        }
    }

    #[test]
    fn the_first_write_prunes_one_time() {
        use std::cell::Cell;
        use std::sync::OnceLock;
        use std::sync::atomic::AtomicU64;

        let dir = test_dir();
        let pruned = OnceLock::new();
        let sequence = AtomicU64::new(0);
        let runs = Cell::new(0_u32);
        let prune = |_: &std::path::Path| runs.set(runs.get() + 1);
        for name in ["a.qoi", "b.qoi"] {
            let path = dir.path().join("cache").join(name);
            write_cache_file_with(
                &path,
                |temp| fs::write(temp, b"x"),
                &pruned,
                &sequence,
                &prune,
            )
            .unwrap();
        }
        assert_eq!(runs.get(), 1, "the first write prunes once, the second not");
    }

    #[test]
    fn the_first_write_does_not_prune_a_second_time() {
        assert!(!prunes_after_write(0));
        assert!(!prunes_after_write(1));
        assert!(prunes_after_write(64));
        assert!(prunes_after_write(128));
    }

    #[test]
    fn a_loop_file_keeps_its_frames_its_times_and_the_size_of_the_still() {
        let dir = test_dir();
        let cache = dir.path().join("cache");
        let source = dir.path().join("clip.mp4");
        fs::write(&source, b"synthetic").unwrap();
        let frames = LoopFrames {
            frames: vec![
                RgbaImage::from_pixel(4, 2, image::Rgba([1, 2, 3, 255])),
                RgbaImage::from_pixel(4, 2, image::Rgba([4, 5, 6, 128])),
            ],
            delays: vec![Duration::from_millis(333), Duration::from_micros(83_333)],
            shown: (8, 4),
        };
        assert!(try_load_loop(&cache, &source, (8, 4)).is_none());
        try_save_loop(&cache, &source, (8, 4), &frames).unwrap();
        assert_eq!(try_load_loop(&cache, &source, (8, 4)), Some(frames));
        assert!(
            try_load_loop(&cache, &source, (16, 8)).is_none(),
            "another size"
        );
        let thumbnail = get_cache_file_path(&cache, &source, (8, 4)).unwrap();
        assert!(!thumbnail.exists(), "the loop has its own key");

        // A cut file reads as a miss and goes.
        let file = fs::read_dir(&cache)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let bytes = fs::read(&file).unwrap();
        fs::write(&file, &bytes[..bytes.len() - 5]).unwrap();
        assert!(try_load_loop(&cache, &source, (8, 4)).is_none());
        assert!(!file.exists());
    }

    #[test]
    fn cache_key_changes_with_dimensions_and_source_size() {
        let dir = test_dir();
        let cache = dir.path().join("cache");
        let source = dir.path().join("source.png");
        fs::write(&source, b"synthetic-a").unwrap();
        let first = get_cache_file_path(&cache, &source, (100, 100)).unwrap();
        let resized = get_cache_file_path(&cache, &source, (200, 100)).unwrap();
        fs::write(&source, b"synthetic-content-b").unwrap();
        let changed = get_cache_file_path(&cache, &source, (100, 100)).unwrap();

        assert_eq!(first.parent(), Some(cache.as_path()));
        assert_ne!(first, resized);
        assert_ne!(first, changed);
        assert_eq!(first.extension().and_then(|ext| ext.to_str()), Some("qoi"));
    }

    /// The key hashes the same input in the same order as the key code before `CacheEntry`,
    /// so the cache files of the same version stay valid.
    #[test]
    fn the_cache_key_hashes_the_input_of_the_old_key_code() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let dir = test_dir();
        let cache = dir.path().join("cache");
        let source = dir.path().join("source.png");
        fs::write(&source, b"synthetic").unwrap();
        let metadata = fs::metadata(&source).unwrap();
        let mtime = metadata
            .modified()
            .unwrap()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap();
        let mut hasher = DefaultHasher::new();
        super::THUMBNAIL_CACHE_VERSION.hash(&mut hasher);
        crate::decode::color::target().hash(&mut hasher);
        fs::canonicalize(&source).unwrap().hash(&mut hasher);
        metadata.len().hash(&mut hasher);
        mtime.as_secs().hash(&mut hasher);
        mtime.subsec_nanos().hash(&mut hasher);
        120_u32.hash(&mut hasher);
        80_u32.hash(&mut hasher);

        assert_eq!(
            get_cache_file_path(&cache, &source, (120, 80)),
            Some(cache.join(format!("{:016x}.qoi", hasher.finish())))
        );
    }

    #[test]
    fn disk_cache_round_trip_and_broken_entries() {
        let dir = test_dir();
        let cache = dir.path().join("cache");
        let source = dir.path().join("a.png");
        RgbaImage::new(4, 4).save(&source).unwrap();
        let mut thumb = RgbaImage::new(2, 3);
        thumb.put_pixel(1, 2, image::Rgba([10, 20, 30, 128]));

        assert!(get_cache_file_path(&cache, &dir.path().join("missing.png"), (1, 1)).is_none());
        assert!(try_load_from_disk_cache(&cache, &source, (2, 3)).is_none());
        try_save_to_disk_cache(&cache, &source, (2, 3), &thumb).unwrap();
        try_save_to_disk_cache(&cache, &source, (2, 3), &thumb).unwrap();
        assert_eq!(
            try_load_from_disk_cache(&cache, &source, (2, 3)),
            Some(thumb.clone()),
            "the pixels come back without a loss"
        );

        let entry = get_cache_file_path(&cache, &source, (2, 3)).unwrap();
        fs::write(&entry, b"not a qoi").unwrap();
        assert!(try_load_from_disk_cache(&cache, &source, (2, 3)).is_none());
        assert!(!entry.exists(), "a broken entry goes away");

        // A cache directory that is a file cannot hold entries.
        let blocked = dir.path().join("blocked");
        fs::write(&blocked, b"file").unwrap();
        assert!(try_save_to_disk_cache(&blocked, &source, (2, 3), &thumb).is_err());
    }

    #[test]
    fn prune_removes_the_oldest_entries_first() {
        let dir = test_dir();
        let cache = dir.path();
        let base = SystemTime::now() - Duration::from_secs(1000);
        for (index, name) in ["old.qoi", "middle.qoi", "new.qoi"].iter().enumerate() {
            let path = cache.join(name);
            fs::write(&path, [0_u8; 100]).unwrap();
            let file = fs::File::options().write(true).open(&path).unwrap();
            file.set_modified(base + Duration::from_secs(index as u64 * 100))
                .unwrap();
        }
        fs::write(cache.join("notes.txt"), [0_u8; 500]).unwrap();
        fs::create_dir(cache.join("folder.qoi")).unwrap();

        prune_disk_cache(cache, 1000);
        assert!(
            cache.join("old.qoi").exists(),
            "under the limit, nothing goes"
        );

        prune_disk_cache(cache, 150);
        assert!(!cache.join("old.qoi").exists());
        assert!(!cache.join("middle.qoi").exists());
        assert!(cache.join("new.qoi").exists());
        assert!(cache.join("notes.txt").exists());

        prune_disk_cache(&cache.join("missing"), 0);
    }

    #[test]
    fn prune_removes_the_loop_files_before_the_stills() {
        let dir = test_dir();
        let cache = dir.path();
        let base = SystemTime::now() - Duration::from_secs(1000);
        let names = ["old.qoi", "old.loop", "new.qoi", "new.loop"];
        for (index, name) in names.iter().enumerate() {
            let path = cache.join(name);
            fs::write(&path, [0_u8; 100]).unwrap();
            let file = fs::File::options().write(true).open(&path).unwrap();
            file.set_modified(base + Duration::from_secs(index as u64 * 100))
                .unwrap();
        }

        prune_disk_cache(cache, 300);
        assert!(!cache.join("old.loop").exists());
        assert!(cache.join("new.loop").exists(), "{names:?}");
        prune_disk_cache(cache, 200);
        assert!(
            !cache.join("new.loop").exists(),
            "a newer loop goes before an old still"
        );
        assert!(cache.join("old.qoi").exists());
        assert!(cache.join("new.qoi").exists());
    }

    #[test]
    fn prune_removes_the_png_files_of_the_older_versions_under_the_limit() {
        let dir = test_dir();
        let cache = dir.path();
        fs::write(cache.join("old.png"), [0_u8; 10]).unwrap();
        fs::write(cache.join("new.qoi"), [0_u8; 10]).unwrap();

        prune_disk_cache(cache, 1000);
        assert!(
            !cache.join("old.png").exists(),
            "an old PNG never hits again"
        );
        assert!(cache.join("new.qoi").exists());
    }

    #[test]
    fn prune_removes_the_temp_files_that_a_quit_left_behind() {
        let dir = test_dir();
        let cache = dir.path();
        let left = cache.join(".abc.png.1.0.tmp");
        let writing = cache.join(".def.png.2.0.tmp");
        let other = cache.join("other.tmp");
        let cube = cache.join("tonemap-hlg-p3-v3.cube.3.0.partial");
        let cube_writing = cache.join("tonemap-pq-p3-v3.cube.4.0.partial");
        for path in [&left, &writing, &other, &cube, &cube_writing] {
            fs::write(path, [0_u8; 10]).unwrap();
        }
        let two_hours_ago = SystemTime::now() - Duration::from_secs(7200);
        for path in [&left, &other, &cube] {
            let file = fs::File::options().write(true).open(path).unwrap();
            file.set_modified(two_hours_ago).unwrap();
        }

        prune_disk_cache(cache, 1000);
        assert!(
            !left.exists(),
            "an old temp file goes, also under the limit"
        );
        assert!(writing.exists(), "a write in progress stays");
        assert!(other.exists(), "only the temp files of the cache go");
        assert!(!cube.exists(), "an old partial cube file goes");
        assert!(cube_writing.exists(), "a cube write in progress stays");
    }
}
