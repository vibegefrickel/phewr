//! The folder of the disk cache. The thumbnail part of the disk cache lives in
//! `tuilibr::pictures::jobs::disk_cache`, the loop files in `tuilibr::pictures::motion::loop_file`.

use std::path::{Path, PathBuf};
use tuilibr::dirs::{OtherVolume, move_once};

/// The cache folder of phewr before the XDG move (2026-11-20): `~/Library/Caches/phewr`.
pub(crate) fn old_cache_dir(home: &Path) -> PathBuf {
    home.join("Library/Caches/phewr")
}

/// Moves the old cache folder to the new one with one rename, before anything opens a file in it.
///
/// `tuilibr::dirs::move_once` moves only when nothing is at the new place, also no dangling link.
/// Across two volumes, the old folder stays and the cache starts empty: a cache needs no copy.
/// Nothing here deletes the old folder. phewr has no log file, so the move gives no note.
pub(crate) fn adopt_old_cache(old: &Path, new: &Path) {
    #[expect(
        clippy::let_underscore_must_use,
        reason = "a failed move leaves the old folder, and the cache starts empty"
    )]
    let _ = move_once(old, new, OtherVolume::Leave);
}

#[cfg(test)]
mod tests {
    use super::{adopt_old_cache, old_cache_dir};
    use std::path::Path;

    #[test]
    fn the_old_cache_folder_is_below_library_caches() {
        assert_eq!(
            old_cache_dir(Path::new("/home/a")),
            Path::new("/home/a/Library/Caches/phewr")
        );
    }

    #[test]
    fn the_old_folder_moves_when_the_new_one_is_missing() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("old");
        let new = root.path().join("cache/phewr");
        std::fs::create_dir(&old).unwrap();
        std::fs::write(old.join("a.qoi"), "x").unwrap();

        adopt_old_cache(&old, &new);

        assert!(!old.exists());
        assert_eq!(std::fs::read_to_string(new.join("a.qoi")).unwrap(), "x");
    }

    #[test]
    fn both_folders_stay_when_both_exist() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("old");
        let new = root.path().join("new");
        std::fs::create_dir(&old).unwrap();
        std::fs::create_dir(&new).unwrap();
        std::fs::write(old.join("a.qoi"), "old").unwrap();
        std::fs::write(new.join("b.qoi"), "new").unwrap();

        adopt_old_cache(&old, &new);

        assert!(old.join("a.qoi").exists());
        assert!(new.join("b.qoi").exists());
        assert!(!new.join("a.qoi").exists());
    }

    #[test]
    fn nothing_happens_when_no_folder_exists() {
        let root = tempfile::tempdir().unwrap();
        let new = root.path().join("new");

        adopt_old_cache(&root.path().join("old"), &new);

        assert!(!new.exists());
    }

    #[test]
    fn a_failed_rename_leaves_the_old_folder() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("old");
        std::fs::create_dir(&old).unwrap();
        std::fs::write(old.join("a.qoi"), "x").unwrap();
        // The parent of the new folder is a file, so the rename fails.
        let blocker = root.path().join("blocker");
        std::fs::write(&blocker, "file").unwrap();
        let new = blocker.join("phewr");

        adopt_old_cache(&old, &new);

        assert!(old.join("a.qoi").exists());
        assert!(!new.exists());
    }
}
