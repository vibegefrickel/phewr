//! The folder and the loop files of the disk cache. The thumbnail part of the disk cache lives in
//! `crate::pictures::jobs::disk_cache`.

mod loop_file;

pub(super) use self::loop_file::{try_load_loop, try_save_loop};

use directories::ProjectDirs;
use std::path::PathBuf;

/// The cache folder of phewr: `~/Library/Caches/phewr`, or a temp folder without a user cache
/// folder. The thumbnails sit at its top since the first version, so the folder must not change:
/// the disk cache of each user stays valid. phewr keeps no subfolder there, so the prune of the
/// module runs. A new app takes `thumbnail_dir_below` of the module instead.
pub(crate) fn default_disk_cache_dir() -> PathBuf {
    ProjectDirs::from("", "", "phewr").map_or_else(
        || std::env::temp_dir().join("phewr-cache"),
        |dirs| dirs.cache_dir().to_path_buf(),
    )
}

#[cfg(test)]
mod tests {
    use super::default_disk_cache_dir;
    use std::path::PathBuf;

    /// The folder of the existing caches: a move drops every thumbnail of the user.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_cache_folder_stays_where_it_was() {
        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME"));
        assert_eq!(default_disk_cache_dir(), home.join("Library/Caches/phewr"));
    }
}
