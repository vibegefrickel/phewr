//! The user configuration (`~/.config/phewr/config.toml`) and the zoom state file.
//!
//! phewr ignores unknown keys. An old file with the removed keys `theme` and `theme_catalog`
//! still loads.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use toml::{Table, Value};
use tuilibr::frame::Layout;
use tuilibr::pictures::decode::color::ColorTarget;
use tuilibr::pictures::kitty::Loops;

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
    /// `[gallery] layout`: the frame of the info pane and the main pane.
    pub(crate) layout: Layout,
    /// The start note for a config file with a syntax error or with bad values.
    pub(crate) start_note: Option<String>,
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
            // phewr starts without boxes (maintainer, 2026-11-19): the grid fills the screen.
            layout: Layout::Minimal,
            start_note: None,
        }
    }
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

    let name = config_path
        .file_name()
        .map_or_else(|| "config".into(), |name| name.to_string_lossy());
    match std::fs::read_to_string(config_path) {
        Ok(contents) => {
            config.start_note = match contents.parse::<Table>() {
                Ok(root) => apply_file(&mut config, &root, &name),
                Err(error) => Some(syntax_note(&name, &contents, &error)),
            };
        }
        // A missing file is normal. Any other read error is a problem that the user must see.
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            config.start_note = Some(read_note(&name, error.kind()));
        }
        Err(_) => {}
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

/// Read each key on its own, so a bad value costs only itself. Return the start note for the
/// problems, if there are any.
fn apply_file(config: &mut Config, root: &Table, name: &str) -> Option<String> {
    let mut problems = Vec::new();
    apply_gallery(config, Section::new(root, "gallery", &mut problems));
    apply_slideshow(config, Section::new(root, "slideshow", &mut problems));
    apply_info(config, Section::new(root, "info", &mut problems));
    apply_video(config, Section::new(root, "video", &mut problems));
    apply_terminal(config, Section::new(root, "terminal", &mut problems));
    (!problems.is_empty()).then(|| format!("{name}: {}, defaults for these", problems.join(", ")))
}

/// One line for the omnibox: a file that exists but cannot be read keeps the defaults.
fn read_note(name: &str, kind: std::io::ErrorKind) -> String {
    let reason = if kind == std::io::ErrorKind::InvalidData {
        "not UTF-8".to_owned()
    } else {
        format!("cannot read ({kind})")
    };
    format!("{name}: {reason}, defaults")
}

/// One line for the omnibox: the file name, the line of the error and the short message.
fn syntax_note(name: &str, contents: &str, error: &toml::de::Error) -> String {
    let line = error.span().map_or(String::new(), |span| {
        let start = span.start.min(contents.len());
        let before = contents.get(..start).unwrap_or_default();
        format!(" line {}", before.matches('\n').count() + 1)
    });
    let message = error.message().lines().next().unwrap_or_default();
    format!("{name}{line}: {message}, defaults")
}

// Each `apply_*` reads one section on its own, so a bad value costs only its key.

/// The `[gallery]` keys.
fn apply_gallery(config: &mut Config, mut section: Section<'_>) {
    if let Some(rows) = section.get("visible_rows", "an integer", visible_rows) {
        config.visible_rows = rows;
        config.configured_visible_rows = rows;
    }
    if let Some(mouse) = section.get("mouse", "a bool", Value::as_bool) {
        config.mouse = mouse;
    }
    if let Some(animate) = section.get("animate", "a bool", Value::as_bool) {
        config.animate = animate;
    }
    if let Some(live_photos) = section.get("live_photos", "a bool", Value::as_bool) {
        config.live_photos = live_photos;
    }
    // `auto` and an unknown value keep the probe.
    if let Some(loops) = section.get("loops", "a string", Value::as_str) {
        config.loops = Loops::parse(loops);
        if config.loops.is_none() && !loops.trim().eq_ignore_ascii_case("auto") {
            section.problems.push(format!("unknown loops {loops:?}"));
        }
    }
    if let Some(layout) = section.get("layout", "a string", Value::as_str) {
        match layout.parse::<Layout>() {
            Ok(layout) => config.layout = layout,
            Err(error) => section
                .problems
                .push(format!("unknown layout {:?}", error.0)),
        }
    }
}

/// The `[slideshow]` keys.
fn apply_slideshow(config: &mut Config, mut section: Section<'_>) {
    if let Some(interval) = section.get("interval", "a number", interval) {
        if interval.is_finite() && interval > 0.0 {
            config.slideshow_interval = interval;
        } else {
            section.problems.push("interval is not above 0".to_owned());
        }
    }
    if let Some(wrap) = section.get("loop", "a bool", Value::as_bool) {
        config.slideshow_loop = wrap;
    }
}

/// The `[info]` keys.
fn apply_info(config: &mut Config, mut section: Section<'_>) {
    if let Some(show) = section.get("show_on_start", "a bool", Value::as_bool) {
        config.info_panel = show;
    }
}

/// The `[video]` keys.
fn apply_video(config: &mut Config, mut section: Section<'_>) {
    if let Some(text) = section.get("player", "a string", Value::as_str) {
        match VideoPlayer::parse(text) {
            Some(player) => config.video_player = player,
            None => section.problems.push(format!("unknown player {text:?}")),
        }
    }
}

/// The `[terminal]` keys.
fn apply_terminal(config: &mut Config, mut section: Section<'_>) {
    if let Some(text) = section.get("colorspace", "a string", Value::as_str) {
        match ColorTarget::parse(text) {
            Some(colorspace) => config.colorspace = colorspace,
            None => section
                .problems
                .push(format!("unknown colorspace {text:?}")),
        }
    }
}

/// An integer of any size, saturated to `u16`, so the clamp of the zoom rows still decides.
fn visible_rows(value: &Value) -> Option<u16> {
    let rows = value.as_integer()?;
    u16::try_from(rows.clamp(0, i64::from(u16::MAX))).ok()
}

/// A float, or an integer such as `interval = 5`.
fn interval(value: &Value) -> Option<f64> {
    value.as_float().or_else(|| {
        value
            .as_integer()
            .and_then(|n| i32::try_from(n).ok())
            .map(f64::from)
    })
}

/// One table of the config file with the list that collects the problems.
struct Section<'a> {
    table: Option<&'a Table>,
    problems: &'a mut Vec<String>,
}

impl<'a> Section<'a> {
    /// A missing section is fine. A section that is not a table is a problem, and all its keys
    /// keep the defaults.
    fn new(root: &'a Table, name: &str, problems: &'a mut Vec<String>) -> Self {
        let table = match root.get(name) {
            None => None,
            Some(Value::Table(table)) => Some(table),
            Some(_) => {
                problems.push(format!("{name} is not a table"));
                None
            }
        };
        Self { table, problems }
    }

    /// The value of `key`, or `None` for a missing key or a value of the wrong type.
    fn get<T>(
        &mut self,
        key: &str,
        kind: &str,
        pick: impl Fn(&'a Value) -> Option<T>,
    ) -> Option<T> {
        let value = self.table?.get(key)?;
        let picked = pick(value);
        if picked.is_none() {
            self.problems.push(format!("{key} is not {kind}"));
        }
        picked
    }
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

    /// An unknown layout keeps the minimal frame and the other keys, and leaves a start note.
    #[test]
    fn reads_gallery_layout() {
        let root = test_root();
        let config_path = root.path().join("config.toml");
        let state_path = root.path().join("state.toml");
        let config = load_at(&config_path, &state_path);
        assert_eq!(
            (config.layout, config.start_note),
            (Layout::Minimal, None),
            "minimal by default"
        );
        for (value, expected, note) in [
            ("minimal", Layout::Minimal, None),
            (" Maximal ", Layout::Maximal, None),
            (
                "boxed",
                Layout::Minimal,
                Some("config.toml: unknown layout \"boxed\", defaults for these"),
            ),
        ] {
            std::fs::write(
                &config_path,
                format!("[gallery]\nlayout = \"{value}\"\nanimate = false\n"),
            )
            .unwrap();
            let config = load_at(&config_path, &state_path);
            assert_eq!(config.layout, expected, "layout = {value:?}");
            assert_eq!(config.start_note.as_deref(), note, "layout = {value:?}");
            assert!(!config.animate, "layout = {value:?} keeps the other keys");
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
                config.slideshow_interval.to_bits(),
                DEFAULT_SLIDESHOW_INTERVAL.to_bits(),
                "interval = {interval}"
            );
        }

        std::fs::write(&config_path, "[gallery\nvisible_rows = 30").unwrap();
        std::fs::write(&state_path, "visible_rows = \"many\"").unwrap();
        let config = load_at(&config_path, &state_path);
        assert_eq!(
            config,
            Config {
                start_note: config.start_note.clone(),
                ..Config::default()
            }
        );
        assert!(config.start_note.is_some());
    }

    fn load_text(text: &str) -> Config {
        let root = test_root();
        let config_path = root.path().join("config.toml");
        std::fs::write(&config_path, text).unwrap();
        load_at(&config_path, &root.path().join("state.toml"))
    }

    /// A wrong type costs only its key. The other keys and sections stay.
    #[test]
    fn wrong_type_keeps_the_other_keys() {
        let config = load_text(
            "[gallery]\nmouse = \"no\"\nanimate = false\nvisible_rows = 6\n[slideshow]\ninterval = \"5\"\nloop = false\n[info]\nshow_on_start = true\n",
        );
        assert!(config.mouse);
        assert!(!config.animate);
        assert_eq!(config.visible_rows, 6);
        assert_eq!(
            config.slideshow_interval.to_bits(),
            DEFAULT_SLIDESHOW_INTERVAL.to_bits()
        );
        assert!(!config.slideshow_loop);
        assert!(config.info_panel);
        assert_eq!(
            config.start_note.as_deref(),
            Some("config.toml: mouse is not a bool, interval is not a number, defaults for these")
        );
    }

    #[test]
    fn integer_interval_and_huge_rows_load() {
        let config =
            load_text("[slideshow]\ninterval = 5\n[gallery]\nvisible_rows = 99999999999\n");
        assert_eq!(config.slideshow_interval.to_bits(), 5.0_f64.to_bits());
        assert_eq!(config.visible_rows, MAX_VISIBLE_ROWS);
        assert_eq!(config.start_note, None);
    }

    #[test]
    fn wrong_typed_section_keeps_the_other_sections() {
        let config = load_text("gallery = 3\n[info]\nshow_on_start = true\n");
        assert_eq!(config.visible_rows, DEFAULT_VISIBLE_ROWS);
        assert!(config.info_panel);
        assert_eq!(
            config.start_note.as_deref(),
            Some("config.toml: gallery is not a table, defaults for these")
        );
    }

    #[test]
    fn syntax_error_gives_defaults_and_a_one_line_note() {
        let config = load_text("[info]\nshow_on_start = true\n\nx = \"open\n");
        assert!(!config.info_panel);
        let note = config.start_note.unwrap();
        assert!(note.starts_with("config.toml line 4: "), "{note}");
        assert!(note.ends_with(", defaults"), "{note}");
        assert!(!note.contains('\n'), "{note}");
    }

    #[test]
    fn several_problems_give_one_note() {
        let config =
            load_text("[gallery]\nmouse = \"no\"\nlayout = \"boxed\"\n[video]\nplayer = 1\n");
        assert_eq!(
            config.start_note.as_deref(),
            Some(
                "config.toml: mouse is not a bool, unknown layout \"boxed\", player is not a string, defaults for these"
            )
        );
    }

    /// A value of the right type that phewr rejects keeps its default and gives a note.
    #[test]
    fn bad_values_of_the_right_type_give_a_note() {
        for (text, part) in [
            ("[slideshow]\ninterval = 0\n", "interval is not above 0"),
            ("[slideshow]\ninterval = -2.0\n", "interval is not above 0"),
            ("[slideshow]\ninterval = nan\n", "interval is not above 0"),
            ("[video]\nplayer = \"vlc\"\n", "unknown player \"vlc\""),
            (
                "[terminal]\ncolorspace = \"rec2020\"\n",
                "unknown colorspace \"rec2020\"",
            ),
            ("[gallery]\nloops = \"sometimes\"\n", "unknown loops"),
        ] {
            let config = load_text(text);
            let note = config.start_note.unwrap_or_default();
            assert!(note.contains(part), "{text:?}: {note}");
            assert!(note.ends_with(", defaults for these"), "{note}");
        }
        for text in [
            "[gallery]\nloops = \"auto\"\n",
            "[gallery]\nloops = \" Ticks \"\n",
        ] {
            assert_eq!(load_text(text).start_note, None, "{text:?}");
        }
    }

    #[test]
    fn unreadable_file_gives_a_note() {
        let root = test_root();
        let state_path = root.path().join("state.toml");
        let config_path = root.path().join("config.toml");
        std::fs::write(&config_path, [0xff_u8, 0xfe, 0x00]).unwrap();
        let config = load_at(&config_path, &state_path);
        assert_eq!(
            config.start_note.as_deref(),
            Some("config.toml: not UTF-8, defaults")
        );
        // A folder in place of the file is another read error.
        let folder = root.path().join("folder.toml");
        std::fs::create_dir(&folder).unwrap();
        let note = load_at(&folder, &state_path).start_note.unwrap_or_default();
        assert!(note.starts_with("folder.toml: cannot read"), "{note}");
        assert!(note.ends_with(", defaults"), "{note}");
    }

    #[test]
    fn good_file_gives_no_note() {
        let config = load_text("[gallery]\nmouse = false\n");
        assert_eq!(config.start_note, None);
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
