//! The thumbnail disk cache: the key, the read, the write and the prune.

use directories::ProjectDirs;
use image::RgbaImage;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// The version in the disk cache key. Change it when the thumbnail output changes. v7: a
/// rotated JPEG gets the DCT scale of its turned box, and RAW and PSD files take their
/// embedded JPEG. v9: each thumbnail is in the color space of the terminal. The key also holds
/// the color target, so a change of `[terminal] colorspace` decodes again. v10: the first frame
/// of a video gets the rotation and the HDR tone mapping. v11: the cache files are QOI, not PNG.
const THUMBNAIL_CACHE_VERSION: &str = "viewr-thumbnail-v11";

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

/// The cache file of `path` at the pixel size `target`. The name hashes the file identity, so a
/// changed file or another size never hits an old thumbnail.
pub(super) fn get_cache_file_path(
    cache_dir: &Path,
    path: &Path,
    target: (u32, u32),
) -> Option<PathBuf> {
    use std::collections::hash_map::DefaultHasher;
    use std::fs;
    use std::hash::{Hash, Hasher};

    let metadata = fs::metadata(path).ok()?;
    let mtime = metadata
        .modified()
        .ok()?
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .ok()?;
    let canonical_path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

    let mut hasher = DefaultHasher::new();
    THUMBNAIL_CACHE_VERSION.hash(&mut hasher);
    crate::decode::color::target().hash(&mut hasher);
    canonical_path.hash(&mut hasher);
    metadata.len().hash(&mut hasher);
    mtime.as_secs().hash(&mut hasher);
    mtime.subsec_nanos().hash(&mut hasher);
    target.0.hash(&mut hasher);
    target.1.hash(&mut hasher);
    let filename = format!("{:016x}.{CACHE_EXTENSION}", hasher.finish());

    Some(cache_dir.join(filename))
}

pub(super) fn try_load_from_disk_cache(
    cache_dir: &Path,
    path: &Path,
    target: (u32, u32),
) -> Option<RgbaImage> {
    let cache_path = get_cache_file_path(cache_dir, path, target)?;
    let bytes = std::fs::read(&cache_path).ok()?;
    if let Ok(image) = image::load_from_memory_with_format(&bytes, CACHE_FORMAT) {
        Some(image.into_rgba8())
    } else {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "a cache file that stays is only disk space, the cache prunes it"
        )]
        let _ = std::fs::remove_file(cache_path);
        None
    }
}

pub(super) fn try_save_to_disk_cache(
    cache_dir: &Path,
    path: &Path,
    target: (u32, u32),
    img: &RgbaImage,
) -> Option<()> {
    use std::fs;
    static CACHE_PRUNED: OnceLock<()> = OnceLock::new();
    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    let cache_path = get_cache_file_path(cache_dir, path, target)?;
    let parent = cache_path.parent()?;
    fs::create_dir_all(parent).ok()?;
    CACHE_PRUNED.get_or_init(|| prune_disk_cache(parent, 512 * 1024 * 1024));
    if cache_path.exists() {
        return Some(());
    }

    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let cache_name = cache_path.file_name()?.to_string_lossy();
    let temp_path = parent.join(format!(
        ".{cache_name}.{}.{}.tmp",
        std::process::id(),
        sequence
    ));
    if img.save_with_format(&temp_path, CACHE_FORMAT).is_err() {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "a cache file that stays is only disk space, the cache prunes it"
        )]
        let _ = fs::remove_file(temp_path);
        return None;
    }
    if fs::rename(&temp_path, &cache_path).is_err() {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "a cache file that stays is only disk space, the cache prunes it"
        )]
        let _ = fs::remove_file(temp_path);
        if !cache_path.exists() {
            return None;
        }
    }
    if sequence.is_multiple_of(64) {
        prune_disk_cache(parent, 512 * 1024 * 1024);
    }
    Some(())
}

/// The age after which a temp file of the cache counts as left behind. A write takes
/// milliseconds, so an older temp file belongs to a viewr that ended during its write.
const STALE_TEMP_AGE: std::time::Duration = std::time::Duration::from_secs(3600);

/// Remove a temp file of `try_save_to_disk_cache` that is older than `STALE_TEMP_AGE`. A quit
/// during a cache write leaves one, and the size prune only counts the cache files.
fn remove_stale_temp_file(entry: &std::fs::DirEntry) {
    let path = entry.path();
    let is_temp = path.extension().and_then(|ext| ext.to_str()) == Some("tmp")
        && entry.file_name().to_string_lossy().starts_with('.');
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
            if extension != Some(CACHE_EXTENSION) {
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

    files.sort_by_key(|(_, _, modified)| *modified);
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
        get_cache_file_path, prune_disk_cache, try_load_from_disk_cache, try_save_to_disk_cache,
    };
    use crate::test_support::test_dir;
    use image::RgbaImage;
    use std::fs;

    use std::time::{Duration, SystemTime};

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
        assert_eq!(
            super::THUMBNAIL_CACHE_VERSION,
            "viewr-thumbnail-v11",
            "the QOI files need a new cache generation"
        );
        assert_eq!(first.extension().and_then(|ext| ext.to_str()), Some("qoi"));
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
        assert_eq!(
            try_save_to_disk_cache(&cache, &source, (2, 3), &thumb),
            Some(())
        );
        assert_eq!(
            try_save_to_disk_cache(&cache, &source, (2, 3), &thumb),
            Some(())
        );
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
        assert!(try_save_to_disk_cache(&blocked, &source, (2, 3), &thumb).is_none());
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
        for path in [&left, &writing, &other] {
            fs::write(path, [0_u8; 10]).unwrap();
        }
        let two_hours_ago = SystemTime::now() - Duration::from_secs(7200);
        for path in [&left, &other] {
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
    }
}
