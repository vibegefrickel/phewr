//! The thumbnail disk cache: the key, the read, the write and the prune.

use directories::ProjectDirs;
use image::RgbaImage;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// The version in the disk cache key. Change it when the thumbnail output changes. v6: the
/// thumbnail box is the whole cell, without padding.
const THUMBNAIL_CACHE_VERSION: &str = "viewr-thumbnail-v6";

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
    canonical_path.hash(&mut hasher);
    metadata.len().hash(&mut hasher);
    mtime.as_secs().hash(&mut hasher);
    mtime.subsec_nanos().hash(&mut hasher);
    target.0.hash(&mut hasher);
    target.1.hash(&mut hasher);
    let filename = format!("{:016x}.png", hasher.finish());

    Some(cache_dir.join(filename))
}

pub(super) fn try_load_from_disk_cache(
    cache_dir: &Path,
    path: &Path,
    target: (u32, u32),
) -> Option<RgbaImage> {
    let cache_path = get_cache_file_path(cache_dir, path, target)?;
    if cache_path.exists() {
        if let Ok(image) = image::open(&cache_path) {
            Some(image.to_rgba8())
        } else {
            #[expect(
                clippy::let_underscore_must_use,
                reason = "a cache file that stays is only disk space, the cache prunes it"
            )]
            let _ = std::fs::remove_file(cache_path);
            None
        }
    } else {
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
    if img
        .save_with_format(&temp_path, image::ImageFormat::Png)
        .is_err()
    {
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

fn prune_disk_cache(cache_dir: &Path, max_bytes: u64) {
    let Ok(entries) = std::fs::read_dir(cache_dir) else {
        return;
    };
    let mut files: Vec<(PathBuf, u64, std::time::SystemTime)> = entries
        .flatten()
        .filter_map(|entry| {
            if entry.path().extension().and_then(|ext| ext.to_str()) != Some("png") {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            metadata.is_file().then(|| {
                (
                    entry.path(),
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
            "viewr-thumbnail-v6",
            "the thumbnail box without padding needs a new cache generation"
        );
    }

    #[test]
    fn disk_cache_round_trip_and_broken_entries() {
        let dir = test_dir();
        let cache = dir.path().join("cache");
        let source = dir.path().join("a.png");
        RgbaImage::new(4, 4).save(&source).unwrap();
        let thumb = RgbaImage::new(2, 3);

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
            try_load_from_disk_cache(&cache, &source, (2, 3))
                .unwrap()
                .dimensions(),
            (2, 3)
        );

        let entry = get_cache_file_path(&cache, &source, (2, 3)).unwrap();
        fs::write(&entry, b"not a png").unwrap();
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
        for (index, name) in ["old.png", "middle.png", "new.png"].iter().enumerate() {
            let path = cache.join(name);
            fs::write(&path, [0_u8; 100]).unwrap();
            let file = fs::File::options().write(true).open(&path).unwrap();
            file.set_modified(base + Duration::from_secs(index as u64 * 100))
                .unwrap();
        }
        fs::write(cache.join("notes.txt"), [0_u8; 500]).unwrap();
        fs::create_dir(cache.join("folder.png")).unwrap();

        prune_disk_cache(cache, 1000);
        assert!(
            cache.join("old.png").exists(),
            "under the limit, nothing goes"
        );

        prune_disk_cache(cache, 150);
        assert!(!cache.join("old.png").exists());
        assert!(!cache.join("middle.png").exists());
        assert!(cache.join("new.png").exists());
        assert!(cache.join("notes.txt").exists());

        prune_disk_cache(&cache.join("missing"), 0);
    }
}
