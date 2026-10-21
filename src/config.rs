//! User configuration (`~/.config/viewr/config.toml`) and the small state
//! file that remembers the grid zoom level.
//!
//! The file has the sections `[gallery]`, `[slideshow]`, `[info]` and `[video]`. viewr ignores
//! unknown keys, so an old file with the removed keys `theme` and `theme_catalog` still loads.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The grid shows this many thumbnail rows until the user zooms.
pub(crate) const DEFAULT_VISIBLE_ROWS: u16 = 4;
/// The largest thumbnails: a 2x2 grid. A larger view is the fullscreen preview.
pub(crate) const MIN_VISIBLE_ROWS: u16 = 2;
pub(crate) const MAX_VISIBLE_ROWS: u16 = 12;
pub(crate) const DEFAULT_SLIDESHOW_INTERVAL: f64 = 3.0;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Config {
    /// The grid zoom level: how many thumbnail rows the terminal shows.
    /// This is the effective level, which the remembered global level can override.
    pub visible_rows: u16,
    /// The zoom level from `[gallery] visible_rows` alone, used by the `0` reset key.
    pub configured_visible_rows: u16,
    /// Seconds between slides.
    pub slideshow_interval: f64,
    /// Wrap around to the first image when the slideshow reaches the end.
    pub slideshow_loop: bool,
    /// Open the info panel on start.
    pub info_panel: bool,
    /// The program that plays a video file.
    pub video_player: VideoPlayer,
}

/// The value of `[video] player`: how viewr plays a video file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum VideoPlayer {
    /// mpv draws into this terminal with `--vo=kitty`.
    Terminal,
    /// mpv opens a fullscreen window of its own, and viewr keeps running.
    #[default]
    Window,
    /// The built-in player of viewr (`video.rs`).
    Builtin,
}

impl VideoPlayer {
    /// Read the config value; an unknown value returns `None`, so the rest of the file still counts.
    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "terminal" => Some(Self::Terminal),
            "window" => Some(Self::Window),
            "builtin" => Some(Self::Builtin),
            _ => None,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            visible_rows: DEFAULT_VISIBLE_ROWS,
            configured_visible_rows: DEFAULT_VISIBLE_ROWS,
            slideshow_interval: DEFAULT_SLIDESHOW_INTERVAL,
            slideshow_loop: true,
            info_panel: false,
            video_player: VideoPlayer::default(),
        }
    }
}

#[derive(Default, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    gallery: GallerySection,
    #[serde(default)]
    slideshow: SlideshowSection,
    #[serde(default)]
    info: InfoSection,
    #[serde(default)]
    video: VideoSection,
}

#[derive(Default, Deserialize)]
struct GallerySection {
    visible_rows: Option<u16>,
}

#[derive(Default, Deserialize)]
struct SlideshowSection {
    interval: Option<f64>,
    #[serde(rename = "loop")]
    wrap: Option<bool>,
}

#[derive(Default, Deserialize)]
struct InfoSection {
    show_on_start: Option<bool>,
}

#[derive(Default, Deserialize)]
struct VideoSection {
    player: Option<String>,
}

/// The largest number of folders that viewr remembers a zoom level for.
const MAX_ZOOM_FOLDERS: usize = 200;

/// Remembered grid zoom, written whenever the user zooms the grid.
#[derive(Default, Deserialize, serde::Serialize)]
struct StateFile {
    /// The last zoom level the user chose anywhere; folders without their own entry fall back
    /// to this value.
    visible_rows: Option<u16>,
    /// Monotonic counter that records the last-used order of per-folder zoom entries.
    #[serde(default)]
    zoom_counter: u64,
    /// Per-folder zoom levels, keyed by the canonical folder path.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    zoom: BTreeMap<PathBuf, ZoomValue>,
}

/// A zoom value in the state file. The simple form `"/path" = 3` is accepted on load; the
/// detailed form with a monotonic order is written so viewr can drop the least recently
/// used folders.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(untagged)]
enum ZoomValue {
    Rows(u16),
    Detailed(ZoomEntry),
}

impl ZoomValue {
    fn rows(&self) -> u16 {
        match self {
            Self::Rows(rows) => *rows,
            Self::Detailed(entry) => entry.rows,
        }
    }

    fn order(&self) -> u64 {
        match self {
            Self::Rows(_) => 0,
            Self::Detailed(entry) => entry.order,
        }
    }
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
struct ZoomEntry {
    rows: u16,
    order: u64,
}

pub(crate) fn load() -> Config {
    let home = home_dir();
    load_at(&config_path(&home), &state_path(&home))
}

pub(crate) fn load_at(config_path: &Path, state_path: &Path) -> Config {
    let mut config = Config::default();

    let file = std::fs::read_to_string(config_path)
        .ok()
        .and_then(|contents| toml::from_str::<ConfigFile>(&contents).ok())
        .unwrap_or_default();
    if let Some(rows) = file.gallery.visible_rows {
        config.visible_rows = rows;
        config.configured_visible_rows = rows;
    }
    if let Some(interval) = file.slideshow.interval
        && interval.is_finite()
        && interval > 0.0
    {
        config.slideshow_interval = interval;
    }
    if let Some(wrap) = file.slideshow.wrap {
        config.slideshow_loop = wrap;
    }
    if let Some(show) = file.info.show_on_start {
        config.info_panel = show;
    }
    if let Some(player) = file.video.player.as_deref().and_then(VideoPlayer::parse) {
        config.video_player = player;
    }

    // The remembered global zoom wins over the configured default until the user
    // presses `0` in a folder, which resets that folder to the configured level.
    let state = read_state(state_path);
    if let Some(rows) = state.visible_rows {
        config.visible_rows = rows;
    }

    // The config key and the saved state both land in the range of the zoom keys.
    config.visible_rows = config
        .visible_rows
        .clamp(MIN_VISIBLE_ROWS, MAX_VISIBLE_ROWS);
    config.configured_visible_rows = config
        .configured_visible_rows
        .clamp(MIN_VISIBLE_ROWS, MAX_VISIBLE_ROWS);
    config
}

/// The state file below the user's home, the default target of the grid-zoom functions.
pub(crate) fn default_state_path() -> PathBuf {
    state_path(&home_dir())
}

/// Read the saved zoom level for `folder` from the state file at `path`.
pub(crate) fn load_folder_zoom(path: &Path, folder: &Path) -> Option<u16> {
    let state = read_state(path);
    state
        .zoom
        .get(folder)
        .map(ZoomValue::rows)
        .map(|rows| rows.clamp(MIN_VISIBLE_ROWS, MAX_VISIBLE_ROWS))
}

/// Read the global saved zoom level from the state file at `path`.
pub(crate) fn load_global_zoom(path: &Path) -> Option<u16> {
    let state = read_state(path);
    state
        .visible_rows
        .map(|rows| rows.clamp(MIN_VISIBLE_ROWS, MAX_VISIBLE_ROWS))
}

/// Remember the zoom level `rows` in the state file at `path`. `folder` gets its own entry,
/// and `rows` is also stored as the global fallback for folders without an entry.
pub(crate) fn save_visible_rows(path: &Path, folder: Option<&Path>, rows: u16) {
    let rows = rows.clamp(MIN_VISIBLE_ROWS, MAX_VISIBLE_ROWS);
    let mut state = read_state(path);
    state.visible_rows = Some(rows);
    if let Some(folder) = folder {
        state.zoom_counter += 1;
        state.zoom.insert(
            folder.to_path_buf(),
            ZoomValue::Detailed(ZoomEntry {
                rows,
                order: state.zoom_counter,
            }),
        );
    }
    prune_oldest_zoom_entries(&mut state);
    write_state(path, &state);
}

/// Forget the zoom level for `folder`. The global fallback stays unchanged.
pub(crate) fn clear_folder_zoom(path: &Path, folder: &Path) {
    let mut state = read_state(path);
    if state.zoom.remove(folder).is_none() {
        return;
    }
    prune_oldest_zoom_entries(&mut state);
    write_state(path, &state);
}

/// Remove the oldest entries when the per-folder table grew past `MAX_ZOOM_FOLDERS`.
fn prune_oldest_zoom_entries(state: &mut StateFile) {
    if state.zoom.len() <= MAX_ZOOM_FOLDERS {
        return;
    }
    let mut entries: Vec<(PathBuf, ZoomValue)> =
        std::mem::take(&mut state.zoom).into_iter().collect();
    entries.sort_by_key(|(_, value)| std::cmp::Reverse(value.order()));
    entries.truncate(MAX_ZOOM_FOLDERS);
    state.zoom = entries.into_iter().collect();
}

fn read_state(path: &Path) -> StateFile {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|contents| toml::from_str(&contents).ok())
        .unwrap_or_default()
}

fn write_state(path: &Path, state: &StateFile) {
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    if let Ok(contents) = toml::to_string(state) {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "the state file is a convenience; viewr runs without it"
        )]
        let _ = std::fs::write(path, contents);
    }
}

pub(crate) fn config_path(home: &Path) -> PathBuf {
    config_path_for(home, std::env::var_os("XDG_CONFIG_HOME"))
}

pub(crate) fn config_path_for(home: &Path, xdg_config_home: Option<OsString>) -> PathBuf {
    xdg_path(xdg_config_home, home, ".config", "viewr/config.toml")
}

fn state_path(home: &Path) -> PathBuf {
    xdg_path(
        std::env::var_os("XDG_STATE_HOME"),
        home,
        ".local/state",
        "viewr/state.toml",
    )
}

/// Join `file` to an absolute XDG base, else to `home/fallback`; the caller reads the variable.
fn xdg_path(base: Option<OsString>, home: &Path, fallback: &str, file: &str) -> PathBuf {
    base.map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(fallback))
        .join(file)
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_root() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn missing_files_yield_defaults() {
        let root = test_root();
        let config = load_at(
            &root.path().join("config.toml"),
            &root.path().join("state.toml"),
        );
        assert_eq!(config, Config::default());
        assert_eq!(config.video_player, VideoPlayer::Window);
    }

    #[test]
    fn config_sections_are_read_and_state_overrides_the_zoom() {
        let root = test_root();
        let config_path = root.path().join("config.toml");
        let state_path = root.path().join("state.toml");
        std::fs::write(
            &config_path,
            "[gallery]\nvisible_rows = 6\n[slideshow]\ninterval = 1.5\nloop = false\n[info]\nshow_on_start = true\n",
        )
        .unwrap();

        let config = load_at(&config_path, &state_path);
        assert_eq!(config.visible_rows, 6);
        assert_eq!(config.slideshow_interval, 1.5);
        assert!(!config.slideshow_loop);
        assert!(config.info_panel);

        write_state(
            &state_path,
            &StateFile {
                visible_rows: Some(2),
                ..StateFile::default()
            },
        );
        let config = load_at(&config_path, &state_path);
        assert_eq!(config.visible_rows, 2);
    }

    #[test]
    fn the_removed_theme_keys_are_ignored() {
        let root = test_root();
        let config_path = root.path().join("config.toml");
        let state_path = root.path().join("state.toml");
        std::fs::write(
            &config_path,
            "theme = \"~/.config/themes/nord.toml\"\ntheme_catalog = \"~/.config/themes/catalog.toml\"\n[gallery]\nvisible_rows = 6\n",
        )
        .unwrap();

        let config = load_at(&config_path, &state_path);

        assert_eq!(
            config,
            Config {
                visible_rows: 6,
                configured_visible_rows: 6,
                ..Config::default()
            }
        );
    }

    #[test]
    fn video_player_is_read_and_unknown_values_keep_the_default() {
        let root = test_root();
        let config_path = root.path().join("config.toml");
        let state_path = root.path().join("state.toml");
        for (value, expected) in [
            ("terminal", VideoPlayer::Terminal),
            ("window", VideoPlayer::Window),
            ("builtin", VideoPlayer::Builtin),
            (" Window ", VideoPlayer::Window),
            ("vlc", VideoPlayer::Window),
        ] {
            std::fs::write(
                &config_path,
                format!("[video]\nplayer = \"{value}\"\n[info]\nshow_on_start = true\n"),
            )
            .unwrap();
            let config = load_at(&config_path, &state_path);
            assert_eq!(config.video_player, expected, "player = {value:?}");
            assert!(config.info_panel, "player = {value:?} keeps the other keys");
        }
    }

    #[test]
    fn an_out_of_range_zoom_is_clamped() {
        let root = test_root();
        let config_path = root.path().join("config.toml");
        assert_eq!((MIN_VISIBLE_ROWS, MAX_VISIBLE_ROWS), (2, 12));
        for (value, expected) in [(0, 2), (1, 2), (13, 12), (999, 12)] {
            std::fs::write(&config_path, format!("[gallery]\nvisible_rows = {value}\n")).unwrap();
            let config = load_at(&config_path, &root.path().join("state.toml"));
            assert_eq!(config.visible_rows, expected, "visible_rows = {value}");
        }
    }

    #[test]
    fn invalid_intervals_and_broken_files_keep_the_defaults() {
        let root = test_root();
        let config_path = root.path().join("config.toml");
        let state_path = root.path().join("state.toml");
        for interval in ["0.0", "-2.0", "nan", "inf"] {
            std::fs::write(
                &config_path,
                format!("[slideshow]\ninterval = {interval}\n"),
            )
            .unwrap();
            let config = load_at(&config_path, &state_path);
            assert_eq!(
                config.slideshow_interval, DEFAULT_SLIDESHOW_INTERVAL,
                "interval = {interval}"
            );
        }

        std::fs::write(&config_path, "[gallery\nvisible_rows = 30").unwrap();
        std::fs::write(&state_path, "visible_rows = \"many\"").unwrap();
        assert_eq!(load_at(&config_path, &state_path), Config::default());
    }

    #[test]
    fn an_old_state_file_with_a_thumb_size_is_ignored() {
        let root = test_root();
        let state_path = root.path().join("state.toml");
        std::fs::write(&state_path, "thumb_cols = 40\nthumb_rows = 12\n").unwrap();
        let config = load_at(&root.path().join("config.toml"), &state_path);
        assert_eq!(config.visible_rows, DEFAULT_VISIBLE_ROWS);
    }

    #[test]
    fn save_and_load_per_folder_zoom_keeps_the_global_fallback() {
        let root = test_root();
        let config_path = root.path().join("config.toml");
        let state_path = root.path().join("state.toml");
        let folder_a = PathBuf::from("/photos/a");
        let folder_b = PathBuf::from("/photos/b");

        save_visible_rows(&state_path, Some(&folder_a), 3);
        assert_eq!(load_folder_zoom(&state_path, &folder_a), Some(3));
        assert_eq!(load_folder_zoom(&state_path, &folder_b), None);
        let config = load_at(&config_path, &state_path);
        assert_eq!(config.visible_rows, 3, "the global fallback is also set");

        save_visible_rows(&state_path, Some(&folder_b), 7);
        assert_eq!(load_folder_zoom(&state_path, &folder_a), Some(3));
        assert_eq!(load_folder_zoom(&state_path, &folder_b), Some(7));
    }

    #[test]
    fn the_configured_default_stays_available_for_the_reset_key() {
        let root = test_root();
        let config_path = root.path().join("config.toml");
        let state_path = root.path().join("state.toml");
        std::fs::write(&config_path, "[gallery]\nvisible_rows = 8\n").unwrap();
        save_visible_rows(&state_path, None, 3);

        let config = load_at(&config_path, &state_path);
        assert_eq!(config.visible_rows, 3);
        assert_eq!(config.configured_visible_rows, 8);
    }

    #[test]
    fn clear_folder_zoom_removes_the_entry_but_keeps_the_global_level() {
        let root = test_root();
        let state_path = root.path().join("state.toml");
        let folder = PathBuf::from("/photos/a");

        save_visible_rows(&state_path, Some(&folder), 5);
        clear_folder_zoom(&state_path, &folder);
        assert_eq!(load_folder_zoom(&state_path, &folder), None);
        let config = load_at(&root.path().join("config.toml"), &state_path);
        assert_eq!(config.visible_rows, 5, "the global fallback stays");
    }

    #[test]
    fn per_folder_zoom_values_outside_the_range_are_clamped() {
        let root = test_root();
        let state_path = root.path().join("state.toml");
        let folder = PathBuf::from("/photos/a");

        std::fs::write(
            &state_path,
            "[zoom]\n\"/photos/a\" = 1\n\"/photos/b\" = 99\n",
        )
        .unwrap();
        assert_eq!(load_folder_zoom(&state_path, &folder), Some(2));
        assert_eq!(
            load_folder_zoom(&state_path, &PathBuf::from("/photos/b")),
            Some(12)
        );
    }

    #[test]
    fn an_old_state_file_without_a_zoom_table_still_loads() {
        let root = test_root();
        let state_path = root.path().join("state.toml");
        std::fs::write(&state_path, "visible_rows = 5\n").unwrap();

        let config = load_at(&root.path().join("config.toml"), &state_path);
        assert_eq!(config.visible_rows, 5);
        assert_eq!(load_folder_zoom(&state_path, &PathBuf::from("/any")), None);
    }

    #[test]
    fn the_zoom_table_drops_the_oldest_folders_past_the_limit() {
        let root = test_root();
        let state_path = root.path().join("state.toml");

        for index in 0..MAX_ZOOM_FOLDERS + 10 {
            let folder = PathBuf::from(format!("/photos/{index}"));
            save_visible_rows(&state_path, Some(&folder), 3);
        }

        let state: StateFile = read_state(&state_path);
        assert_eq!(state.zoom.len(), MAX_ZOOM_FOLDERS);
        assert!(
            !state.zoom.contains_key(&PathBuf::from("/photos/0")),
            "the oldest folders are dropped"
        );
        assert!(
            state
                .zoom
                .contains_key(&PathBuf::from(format!("/photos/{}", MAX_ZOOM_FOLDERS + 9))),
            "the newest folders stay"
        );
    }

    #[test]
    fn write_state_gives_up_without_a_usable_parent() {
        let root = test_root();
        let blocker = root.path().join("file");
        std::fs::write(&blocker, "not a directory").unwrap();
        let state = StateFile {
            visible_rows: Some(6),
            ..StateFile::default()
        };

        write_state(&blocker.join("state.toml"), &state);
        assert!(blocker.is_file());

        write_state(Path::new("/"), &state);
    }

    #[test]
    fn xdg_path_uses_only_an_absolute_base() {
        let home = Path::new("/home/synthetic");
        assert_eq!(
            xdg_path(None, home, ".config", "viewr/config.toml"),
            home.join(".config/viewr/config.toml")
        );
        assert_eq!(
            xdg_path(
                Some(OsString::from("relative/config")),
                home,
                ".config",
                "viewr/config.toml"
            ),
            home.join(".config/viewr/config.toml")
        );
        assert_eq!(
            xdg_path(
                Some(OsString::from("/xdg/state")),
                home,
                ".local/state",
                "viewr/state.toml"
            ),
            PathBuf::from("/xdg/state/viewr/state.toml")
        );
    }

    #[test]
    fn config_and_state_paths_end_in_the_viewr_files() {
        let home = Path::new("/home/synthetic");
        assert!(config_path(home).ends_with("viewr/config.toml"));
        assert!(state_path(home).ends_with("viewr/state.toml"));
    }
}
