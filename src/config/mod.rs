//! The user configuration (`~/.config/phewr/config.toml`) and the zoom state file.
//!
//! phewr ignores unknown keys. An old file with the removed keys `theme` and `theme_catalog`
//! still loads.

use crate::pictures::decode::color::ColorTarget;
use crate::pictures::kitty::Loops;
use serde::Deserialize;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

mod state;

use state::read_state;
pub(crate) use state::{StateChange, StateWriter};
#[cfg(test)]
pub(crate) use state::{load_folder_zoom, load_global_zoom, save_visible_rows};

/// The thumbnail rows until the user zooms.
pub(crate) const DEFAULT_VISIBLE_ROWS: u16 = 4;
/// The largest thumbnails: a 2x2 grid.
pub(crate) const MIN_VISIBLE_ROWS: u16 = 2;
pub(crate) const MAX_VISIBLE_ROWS: u16 = 12;
pub(crate) const DEFAULT_SLIDESHOW_INTERVAL: f64 = 3.0;

#[derive(Debug, Clone, PartialEq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is one independent key of the config file"
)]
pub(crate) struct Config {
    /// The effective zoom level. The remembered global level overrides the config key.
    pub(crate) visible_rows: u16,
    /// The zoom level of `[gallery] visible_rows` alone, for the `0` reset key.
    pub(crate) configured_visible_rows: u16,
    pub(crate) slideshow_interval: f64,
    /// Wrap around to the first image at the end of the slideshow.
    pub(crate) slideshow_loop: bool,
    pub(crate) info_panel: bool,
    pub(crate) video_player: VideoPlayer,
    /// Capture the mouse. Off, the terminal keeps its own selection and scrolling.
    pub(crate) mouse: bool,
    /// Play a loop in each visible video and animated cell.
    pub(crate) animate: bool,
    /// Hide the motion `.mov` of each Live Photo still of a folder. Off, both cells show.
    pub(crate) live_photos: bool,
    /// `[gallery] loops`. `None` (`auto`) lets the start probe decide. `PHEWR_KITTY_LOOPS` wins.
    pub(crate) loops: Option<Loops>,
    /// The color space of the terminal pixels.
    pub(crate) colorspace: ColorTarget,
}

/// The value of `[video] player`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum VideoPlayer {
    /// The phewr player draws the frames into this terminal, without sound.
    #[default]
    Terminal,
    /// mpv opens its own fullscreen window, with sound.
    Window,
}

impl VideoPlayer {
    /// Read the config value. An unknown value returns `None`, and the rest of the file counts.
    /// `builtin` is the old name of `terminal`.
    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "terminal" | "builtin" => Some(Self::Terminal),
            "window" => Some(Self::Window),
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
            mouse: true,
            animate: true,
            live_photos: true,
            loops: None,
            colorspace: ColorTarget::default(),
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
    #[serde(default)]
    terminal: TerminalSection,
}

#[derive(Default, Deserialize)]
struct GallerySection {
    visible_rows: Option<u16>,
    mouse: Option<bool>,
    animate: Option<bool>,
    live_photos: Option<bool>,
    loops: Option<String>,
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

#[derive(Default, Deserialize)]
struct TerminalSection {
    colorspace: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Paths {
    pub(crate) config: PathBuf,
    pub(crate) state: PathBuf,
}

impl Paths {
    /// Reads `HOME`, `XDG_CONFIG_HOME` and `XDG_STATE_HOME` at each call.
    pub(crate) fn from_env() -> Self {
        Self::from_vars(
            &home_dir(),
            std::env::var_os("XDG_CONFIG_HOME"),
            std::env::var_os("XDG_STATE_HOME"),
        )
    }

    /// Takes the values of the variables. A test needs no environment.
    fn from_vars(
        home: &Path,
        xdg_config_home: Option<OsString>,
        xdg_state_home: Option<OsString>,
    ) -> Self {
        Self {
            config: xdg_path(xdg_config_home, home, ".config", "phewr/config.toml"),
            state: xdg_path(xdg_state_home, home, ".local/state", "phewr/state.toml"),
        }
    }
}

pub(crate) fn load(paths: &Paths) -> Config {
    load_at(&paths.config, &paths.state)
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
    if let Some(mouse) = file.gallery.mouse {
        config.mouse = mouse;
    }
    if let Some(animate) = file.gallery.animate {
        config.animate = animate;
    }
    if let Some(live_photos) = file.gallery.live_photos {
        config.live_photos = live_photos;
    }
    // `auto` and an unknown value keep the probe.
    if let Some(loops) = file.gallery.loops.as_deref() {
        config.loops = Loops::parse(loops);
    }
    if let Some(show) = file.info.show_on_start {
        config.info_panel = show;
    }
    if let Some(player) = file.video.player.as_deref().and_then(VideoPlayer::parse) {
        config.video_player = player;
    }
    if let Some(colorspace) = file
        .terminal
        .colorspace
        .as_deref()
        .and_then(ColorTarget::parse)
    {
        config.colorspace = colorspace;
    }

    // The remembered global zoom wins over the configured default. The `0` key resets a folder to
    // the configured level.
    let state = read_state(state_path);
    if let Some(rows) = state.visible_rows {
        config.visible_rows = rows;
    }

    config.visible_rows = config
        .visible_rows
        .clamp(MIN_VISIBLE_ROWS, MAX_VISIBLE_ROWS);
    config.configured_visible_rows = config
        .configured_visible_rows
        .clamp(MIN_VISIBLE_ROWS, MAX_VISIBLE_ROWS);
    config
}

/// Join `file` to an absolute XDG base, else to `home/fallback`.
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
    use super::state::{StateFile, write_state};
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn paths_follow_absolute_xdg_values() {
        let home = Path::new("/home/a");
        let fallback = Paths::from_vars(home, None, None);
        assert_eq!(fallback.config, home.join(".config/phewr/config.toml"));
        assert_eq!(fallback.state, home.join(".local/state/phewr/state.toml"));

        let xdg = Paths::from_vars(home, Some("/x/c".into()), Some("/x/s".into()));
        assert_eq!(xdg.config, Path::new("/x/c/phewr/config.toml"));
        assert_eq!(xdg.state, Path::new("/x/s/phewr/state.toml"));

        let relative = Paths::from_vars(home, Some("c".into()), Some("s".into()));
        assert_eq!(relative, fallback);
    }

    pub(super) fn test_root() -> TempDir {
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
        assert_eq!(config.video_player, VideoPlayer::Terminal);
    }

    #[test]
    fn reads_sections_and_state_zoom() {
        let root = test_root();
        let config_path = root.path().join("config.toml");
        let state_path = root.path().join("state.toml");
        std::fs::write(
            &config_path,
            "[gallery]\nvisible_rows = 6\nmouse = false\nanimate = false\nlive_photos = false\n[slideshow]\ninterval = 1.5\nloop = false\n[info]\nshow_on_start = true\n",
        )
        .unwrap();

        let config = load_at(&config_path, &state_path);
        assert_eq!(config.visible_rows, 6);
        assert!(!config.mouse);
        assert!(!config.animate);
        assert!(Config::default().animate, "the loops are on by default");
        assert!(!config.live_photos);
        assert!(Config::default().live_photos, "the pairs are on by default");
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
    fn ignores_removed_theme_keys() {
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
    fn reads_video_player() {
        let root = test_root();
        let config_path = root.path().join("config.toml");
        let state_path = root.path().join("state.toml");
        for (value, expected) in [
            ("terminal", VideoPlayer::Terminal),
            ("window", VideoPlayer::Window),
            ("builtin", VideoPlayer::Terminal),
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
    }

    #[test]
    fn loops_key_fixes_loop_path() {
        let root = test_root();
        let config_path = root.path().join("config.toml");
        let state_path = root.path().join("state.toml");
        assert_eq!(
            load_at(&config_path, &state_path).loops,
            None,
            "auto by default"
        );
        for (value, expected) in [
            ("animation", Some(Loops::Animation)),
            (" Ticks ", Some(Loops::Ticks)),
            ("auto", None),
            ("sometimes", None),
        ] {
            std::fs::write(
                &config_path,
                format!("[gallery]\nloops = \"{value}\"\nanimate = false\n"),
            )
            .unwrap();
            let config = load_at(&config_path, &state_path);
            assert_eq!(config.loops, expected, "loops = {value:?}");
            assert!(!config.animate, "loops = {value:?} keeps the other keys");
        }
    }

    #[test]
    fn reads_terminal_colorspace() {
        let root = test_root();
        let config_path = root.path().join("config.toml");
        let state_path = root.path().join("state.toml");
        for (value, expected) in [
            ("srgb", ColorTarget::Srgb),
            ("display-p3", ColorTarget::DisplayP3),
            ("rec2020", ColorTarget::DisplayP3),
        ] {
            std::fs::write(
                &config_path,
                format!("[terminal]\ncolorspace = \"{value}\"\n"),
            )
            .unwrap();
            let config = load_at(&config_path, &state_path);
            assert_eq!(config.colorspace, expected, "colorspace = {value:?}");
        }
    }

    #[test]
    fn clamps_out_of_range_zoom() {
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
    fn bad_intervals_and_files_keep_defaults() {
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
    fn ignores_old_thumb_size_state() {
        let root = test_root();
        let state_path = root.path().join("state.toml");
        std::fs::write(&state_path, "thumb_cols = 40\nthumb_rows = 12\n").unwrap();
        let config = load_at(&root.path().join("config.toml"), &state_path);
        assert_eq!(config.visible_rows, DEFAULT_VISIBLE_ROWS);
    }

    #[test]
    fn xdg_path_needs_absolute_base() {
        let home = Path::new("/home/synthetic");
        assert_eq!(
            xdg_path(None, home, ".config", "phewr/config.toml"),
            home.join(".config/phewr/config.toml")
        );
        assert_eq!(
            xdg_path(
                Some(OsString::from("relative/config")),
                home,
                ".config",
                "phewr/config.toml"
            ),
            home.join(".config/phewr/config.toml")
        );
        assert_eq!(
            xdg_path(
                Some(OsString::from("/xdg/state")),
                home,
                ".local/state",
                "phewr/state.toml"
            ),
            PathBuf::from("/xdg/state/phewr/state.toml")
        );
    }
}
