//! User configuration (`~/.config/viewr/config.toml`) and the small state
//! file that remembers the last used thumbnail size.
//!
//! Theme-related keys (`theme`, `theme_catalog`) are read by `theme.rs` from
//! the same file; this module only handles the gallery, slideshow, info and video sections.

use serde::Deserialize;
use std::path::{Path, PathBuf};

pub(crate) const DEFAULT_THUMB_COLS: u16 = 20;
pub(crate) const DEFAULT_THUMB_ROWS: u16 = 8;
pub(crate) const DEFAULT_SLIDESHOW_INTERVAL: f64 = 3.0;

pub(crate) const MIN_THUMB_COLS: u16 = 8;
pub(crate) const MAX_THUMB_COLS: u16 = 120;
pub(crate) const MIN_THUMB_ROWS: u16 = 3;
pub(crate) const MAX_THUMB_ROWS: u16 = 60;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Config {
    /// Thumbnail cell size in terminal cells (border included).
    pub thumb_cols: u16,
    pub thumb_rows: u16,
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
    #[default]
    Terminal,
    /// mpv opens a window of its own, and viewr keeps running.
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
            thumb_cols: DEFAULT_THUMB_COLS,
            thumb_rows: DEFAULT_THUMB_ROWS,
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
    thumb_cols: Option<u16>,
    thumb_rows: Option<u16>,
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

/// Remembered thumbnail size, written whenever the user zooms the grid.
#[derive(Default, Deserialize, serde::Serialize)]
struct StateFile {
    thumb_cols: Option<u16>,
    thumb_rows: Option<u16>,
}

pub(crate) fn load() -> Config {
    let home = home_dir();
    load_at(&config_path(&home), &state_path(&home))
}

fn load_at(config_path: &Path, state_path: &Path) -> Config {
    let mut config = Config::default();

    let file = std::fs::read_to_string(config_path)
        .ok()
        .and_then(|contents| toml::from_str::<ConfigFile>(&contents).ok())
        .unwrap_or_default();
    if let Some(cols) = file.gallery.thumb_cols {
        config.thumb_cols = cols;
    }
    if let Some(rows) = file.gallery.thumb_rows {
        config.thumb_rows = rows;
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

    // The remembered size wins over the configured default until the user
    // presses `0`, which resets to the configured size and forgets the state.
    let state = std::fs::read_to_string(state_path)
        .ok()
        .and_then(|contents| toml::from_str::<StateFile>(&contents).ok())
        .unwrap_or_default();
    if let (Some(cols), Some(rows)) = (state.thumb_cols, state.thumb_rows) {
        config.thumb_cols = cols;
        config.thumb_rows = rows;
    }

    config.thumb_cols = config.thumb_cols.clamp(MIN_THUMB_COLS, MAX_THUMB_COLS);
    config.thumb_rows = config.thumb_rows.clamp(MIN_THUMB_ROWS, MAX_THUMB_ROWS);
    config
}

/// Remember `cols` x `rows` as the thumbnail size for the next start.
pub(crate) fn save_thumb_size(cols: u16, rows: u16) {
    let path = state_path(&home_dir());
    let state = StateFile {
        thumb_cols: Some(cols),
        thumb_rows: Some(rows),
    };
    write_state(&path, &state);
}

/// Forget the remembered thumbnail size.
pub(crate) fn clear_thumb_size() {
    let path = state_path(&home_dir());
    if path.exists() {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "a state file that stays only brings the old size back"
        )]
        let _ = std::fs::remove_file(path);
    }
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

fn config_path(home: &Path) -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".config"))
        .join("viewr/config.toml")
}

fn state_path(home: &Path) -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".local/state"))
        .join("viewr/state.toml")
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn test_root() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "viewr-config-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn missing_files_yield_defaults() {
        let root = test_root();
        let config = load_at(&root.join("config.toml"), &root.join("state.toml"));
        assert_eq!(config, Config::default());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn config_sections_are_read_and_state_overrides_thumb_size() {
        let root = test_root();
        let config_path = root.join("config.toml");
        let state_path = root.join("state.toml");
        std::fs::write(
            &config_path,
            "theme = \"ignored-here\"\n[gallery]\nthumb_cols = 30\nthumb_rows = 12\n[slideshow]\ninterval = 1.5\nloop = false\n[info]\nshow_on_start = true\n",
        )
        .unwrap();

        let config = load_at(&config_path, &state_path);
        assert_eq!((config.thumb_cols, config.thumb_rows), (30, 12));
        assert_eq!(config.slideshow_interval, 1.5);
        assert!(!config.slideshow_loop);
        assert!(config.info_panel);

        write_state(
            &state_path,
            &StateFile {
                thumb_cols: Some(16),
                thumb_rows: Some(6),
            },
        );
        let config = load_at(&config_path, &state_path);
        assert_eq!((config.thumb_cols, config.thumb_rows), (16, 6));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn video_player_is_read_and_unknown_values_keep_the_default() {
        let root = test_root();
        let config_path = root.join("config.toml");
        let state_path = root.join("state.toml");
        for (value, expected) in [
            ("terminal", VideoPlayer::Terminal),
            ("window", VideoPlayer::Window),
            ("builtin", VideoPlayer::Builtin),
            (" Window ", VideoPlayer::Window),
            ("vlc", VideoPlayer::Terminal),
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
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn out_of_range_sizes_are_clamped() {
        let root = test_root();
        let config_path = root.join("config.toml");
        std::fs::write(
            &config_path,
            "[gallery]\nthumb_cols = 2\nthumb_rows = 999\n",
        )
        .unwrap();
        let config = load_at(&config_path, &root.join("state.toml"));
        assert_eq!(config.thumb_cols, MIN_THUMB_COLS);
        assert_eq!(config.thumb_rows, MAX_THUMB_ROWS);
        std::fs::remove_dir_all(root).unwrap();
    }
}
