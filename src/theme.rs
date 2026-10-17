use ratatui::style::{Color, Modifier, Style};
use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Theme {
    pub background: Style,
    pub border: Style,
    pub border_selected: Style,
    pub border_marked: Style,
    pub title: Style,
    pub label: Style,
    pub label_selected: Style,
    pub status_bar: Style,
    pub status_bar_error: Style,
    pub status_bar_dim: Style,
    pub mode_normal: Style,
    pub mode_search: Style,
    pub search_input: Style,
    pub search_cursor: Style,
    pub picker_backdrop: Style,
    pub picker_surface: Style,
    pub picker_chrome: Style,
    pub picker_selection: Style,
    pub popup_border: Style,
    pub popup_title: Style,
    pub popup_text: Style,
    pub popup_key: Style,
    pub popup_desc: Style,
}

#[derive(Debug, Clone)]
pub(crate) struct NamedTheme {
    pub name: String,
    pub path: Option<PathBuf>,
    pub theme: Theme,
}

pub(crate) struct ThemeSet {
    pub themes: Vec<NamedTheme>,
    pub selected: usize,
}

#[derive(Default, Deserialize)]
struct AppConfig {
    theme: Option<String>,
    theme_catalog: Option<String>,
}

#[derive(Default, Deserialize)]
struct ThemeCatalog {
    #[serde(default)]
    themes: Vec<String>,
}

#[derive(Default, Deserialize)]
struct ThemeFile {
    #[serde(default)]
    colors: BTreeMap<String, String>,
    #[serde(default)]
    ui: BTreeMap<String, String>,
}

pub(crate) fn load_themes(cli_theme: Option<&str>) -> Result<ThemeSet, String> {
    let home = home_dir();
    load_themes_at(&home, cli_theme)
}

fn load_themes_at(home: &Path, cli_theme: Option<&str>) -> Result<ThemeSet, String> {
    let config_path = home.join(".config/viewr/config.toml");
    let config = std::fs::read_to_string(config_path)
        .ok()
        .and_then(|contents| toml::from_str::<AppConfig>(&contents).ok())
        .unwrap_or_default();

    let mut themes = Vec::new();
    let mut loaded_paths = HashSet::new();
    if let Some(catalog_path) = config.theme_catalog.as_deref() {
        let catalog_path = expand_home(home, catalog_path);
        for path in load_catalog_paths(&catalog_path, home) {
            if let Some(theme) = load_theme_path(&path)
                && loaded_paths.insert(path.clone())
            {
                themes.push(theme);
            }
        }
    }

    let configured = cli_theme.or(config.theme.as_deref());
    let strict = cli_theme.is_some();
    let selected = configured
        .and_then(|value| select_theme(value, home, &mut themes, &mut loaded_paths))
        .or_else(|| (!themes.is_empty()).then_some(0));

    if strict && selected.is_none() {
        let available = themes
            .iter()
            .map(|theme| theme.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "theme '{value}' is not a readable file or configured catalog entry{}",
            if available.is_empty() {
                String::new()
            } else {
                format!("; available: {available}")
            },
            value = configured.unwrap_or_default(),
        ));
    }

    let selected = selected.unwrap_or_else(|| {
        themes.push(NamedTheme {
            name: "tokyo night moon fallback".to_owned(),
            path: None,
            theme: Theme::fallback(),
        });
        0
    });

    Ok(ThemeSet { themes, selected })
}

fn select_theme(
    configured: &str,
    home: &Path,
    themes: &mut Vec<NamedTheme>,
    loaded_paths: &mut HashSet<PathBuf>,
) -> Option<usize> {
    let path = expand_home(home, configured);
    if path.is_file() {
        if let Some(index) = themes
            .iter()
            .position(|theme| theme.path.as_ref() == Some(&path))
        {
            return Some(index);
        }
        let theme = load_theme_path(&path)?;
        loaded_paths.insert(path);
        themes.push(theme);
        return Some(themes.len() - 1);
    }

    let requested = normalized_name(configured);
    themes.iter().position(|theme| {
        normalized_name(&theme.name) == requested
            || (requested == "catppuccin" && normalized_name(&theme.name) == "catppuccinmocha")
    })
}

fn load_catalog_paths(catalog_path: &Path, home: &Path) -> Vec<PathBuf> {
    let Ok(contents) = std::fs::read_to_string(catalog_path) else {
        return Vec::new();
    };
    let Ok(catalog) = toml::from_str::<ThemeCatalog>(&contents) else {
        return Vec::new();
    };
    catalog
        .themes
        .iter()
        .map(|path| expand_home(home, path))
        .collect()
}

fn load_theme_path(path: &Path) -> Option<NamedTheme> {
    let contents = std::fs::read_to_string(path).ok()?;
    let file = toml::from_str::<ThemeFile>(&contents).ok()?;
    let theme = Theme::from_file(&file)?;
    Some(NamedTheme {
        name: theme_name(path),
        path: Some(path.to_path_buf()),
        theme,
    })
}

impl Theme {
    fn from_file(file: &ThemeFile) -> Option<Self> {
        let background = resolve(file, &["background"], &["bg", "base"])?;
        let background_dark = resolve(
            file,
            &["background_dark", "popup_bg"],
            &["bg_dark", "mantle", "bg"],
        )
        .unwrap_or(background);
        let background_deep = resolve(
            file,
            &["background_deep"],
            &["bg_dark1", "crust", "bg_dark", "base"],
        )
        .unwrap_or(background_dark);
        let background_highlight = resolve(
            file,
            &["cursor_bg"],
            &["bg_highlight", "surface0", "surface1"],
        )
        .unwrap_or(background_dark);
        let text = resolve(file, &["text"], &["fg", "text"])?;
        let text_dim = resolve(
            file,
            &["text_dim"],
            &["comment", "fg_dim", "subtext0", "overlay0"],
        )
        .unwrap_or(text);
        let border =
            resolve(file, &["border"], &["fg_gutter", "surface1", "overlay0"]).unwrap_or(text_dim);
        let accent =
            resolve(file, &["accent"], &["magenta", "mauve", "purple", "pink"]).unwrap_or(text);
        let heading = resolve(file, &["heading"], &["blue", "sapphire"]).unwrap_or(accent);
        let key = resolve(file, &["key"], &["cyan", "sky"]).unwrap_or(heading);
        let green = resolve(file, &[], &["green", "teal"]).unwrap_or(accent);
        let search = resolve(file, &[], &["teal", "green1", "green"]).unwrap_or(green);
        let error = resolve(file, &["error"], &["red", "maroon"]).unwrap_or(accent);
        let cursor =
            resolve(file, &["picker_loading"], &["blue5", "cyan", "sky", "blue"]).unwrap_or(key);
        let popup_border =
            resolve(file, &["picker_border"], &["blue7", "lavender", "surface1"]).unwrap_or(accent);
        let popup_title = resolve(file, &[], &["yellow", "peach"]).unwrap_or(accent);

        Some(Self {
            background: Style::default().bg(background),
            border: Style::default().fg(border),
            border_selected: Style::default().fg(accent).add_modifier(Modifier::BOLD),
            border_marked: Style::default().fg(green).add_modifier(Modifier::BOLD),
            title: Style::default().fg(heading).add_modifier(Modifier::BOLD),
            label: Style::default().fg(text),
            label_selected: Style::default().fg(accent),
            status_bar: Style::default().fg(text).bg(background_dark),
            status_bar_error: Style::default().fg(background_deep).bg(error),
            status_bar_dim: Style::default().fg(text_dim).bg(background_dark),
            mode_normal: Style::default()
                .fg(background_deep)
                .bg(heading)
                .add_modifier(Modifier::BOLD),
            mode_search: Style::default()
                .fg(background_deep)
                .bg(search)
                .add_modifier(Modifier::BOLD),
            search_input: Style::default().fg(text).bg(background_highlight),
            search_cursor: Style::default().fg(background_deep).bg(cursor),
            picker_backdrop: Style::default().bg(background_deep),
            picker_surface: Style::default().fg(text).bg(background),
            picker_chrome: Style::default().fg(text_dim).bg(background_dark),
            picker_selection: Style::default().fg(text).bg(background_highlight),
            popup_border: Style::default().fg(popup_border),
            popup_title: Style::default()
                .fg(popup_title)
                .add_modifier(Modifier::BOLD),
            popup_text: Style::default().fg(text),
            popup_key: Style::default().fg(key).add_modifier(Modifier::BOLD),
            popup_desc: Style::default().fg(text_dim),
        })
    }

    pub(crate) fn fallback() -> Self {
        let bg = Color::Rgb(0x22, 0x24, 0x36);
        let bg_dark1 = Color::Rgb(0x19, 0x1b, 0x29);
        let fg = Color::Rgb(0xc8, 0xd3, 0xf5);
        let fg_dark = Color::Rgb(0x82, 0x8b, 0xb8);
        let bg_dark = Color::Rgb(0x1e, 0x20, 0x30);
        let bg_highlight = Color::Rgb(0x2f, 0x33, 0x4d);
        let blue = Color::Rgb(0x82, 0xaa, 0xff);
        let blue5 = Color::Rgb(0x89, 0xdd, 0xff);
        let yellow = Color::Rgb(0xff, 0xc7, 0x77);
        let red = Color::Rgb(0xff, 0x75, 0x7f);
        let green = Color::Rgb(0xc3, 0xe8, 0x8d);
        let teal = Color::Rgb(0x4f, 0xd6, 0xbe);
        let magenta = Color::Rgb(0xc0, 0x99, 0xff);
        let comment = Color::Rgb(0x63, 0x6d, 0xa6);
        let dark3 = Color::Rgb(0x54, 0x5c, 0x7e);

        Self {
            background: Style::default().bg(bg),
            border: Style::default().fg(dark3),
            border_selected: Style::default().fg(magenta).add_modifier(Modifier::BOLD),
            border_marked: Style::default().fg(green).add_modifier(Modifier::BOLD),
            title: Style::default().fg(blue).add_modifier(Modifier::BOLD),
            label: Style::default().fg(fg),
            label_selected: Style::default().fg(magenta),
            status_bar: Style::default().fg(fg).bg(bg_dark),
            status_bar_error: Style::default().fg(bg_dark1).bg(red),
            status_bar_dim: Style::default().fg(comment).bg(bg_dark),
            mode_normal: Style::default()
                .fg(bg_dark1)
                .bg(blue)
                .add_modifier(Modifier::BOLD),
            mode_search: Style::default()
                .fg(bg_dark1)
                .bg(teal)
                .add_modifier(Modifier::BOLD),
            search_input: Style::default().fg(fg).bg(bg_highlight),
            search_cursor: Style::default().fg(bg_dark1).bg(blue5),
            picker_backdrop: Style::default().bg(bg_dark1),
            picker_surface: Style::default().fg(fg).bg(bg),
            picker_chrome: Style::default().fg(fg_dark).bg(bg_dark),
            picker_selection: Style::default().fg(fg).bg(bg_highlight),
            popup_border: Style::default().fg(magenta),
            popup_title: Style::default().fg(yellow).add_modifier(Modifier::BOLD),
            popup_text: Style::default().fg(fg),
            popup_key: Style::default().fg(blue).add_modifier(Modifier::BOLD),
            popup_desc: Style::default().fg(fg_dark),
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::fallback()
    }
}

fn resolve(file: &ThemeFile, ui_names: &[&str], color_names: &[&str]) -> Option<Color> {
    ui_names
        .iter()
        .filter_map(|name| file.ui.get(*name))
        .find_map(|value| resolve_value(value, &file.colors))
        .or_else(|| {
            color_names
                .iter()
                .filter_map(|name| file.colors.get(*name))
                .find_map(|value| parse_hex(value))
        })
}

fn resolve_value(value: &str, colors: &BTreeMap<String, String>) -> Option<Color> {
    parse_hex(value).or_else(|| colors.get(value).and_then(|value| parse_hex(value)))
}

fn parse_hex(value: &str) -> Option<Color> {
    let value = value.strip_prefix('#')?;
    if value.len() != 6 {
        return None;
    }
    Some(Color::Rgb(
        u8::from_str_radix(value.get(0..2)?, 16).ok()?,
        u8::from_str_radix(value.get(2..4)?, 16).ok()?,
        u8::from_str_radix(value.get(4..6)?, 16).ok()?,
    ))
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from)
}

fn expand_home(home: &Path, configured_path: &str) -> PathBuf {
    configured_path
        .strip_prefix("~/")
        .map_or_else(|| PathBuf::from(configured_path), |rest| home.join(rest))
}

fn theme_name(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("theme")
        .replace('-', " ")
}

fn normalized_name(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const SHARED_THEME: &str = r##"
[colors]
bg = "#222436"
bg_dark = "#1e2030"
bg_dark1 = "#191b29"
bg_highlight = "#2f334d"
fg = "#c8d3f5"
comment = "#636da6"
fg_gutter = "#3b4261"
red = "#ff757f"
yellow = "#ffc777"
green = "#c3e88d"
blue = "#82aaff"
blue5 = "#89ddff"
magenta = "#c099ff"
teal = "#4fd6be"

[ui]
background = "bg"
background_dark = "bg_dark"
background_deep = "bg_dark1"
border = "fg_gutter"
accent = "magenta"
key = "blue5"
heading = "blue"
text = "fg"
text_dim = "comment"
cursor_bg = "bg_highlight"
error = "red"
"##;

    #[test]
    fn shared_theme_maps_viewr_roles() {
        let file = toml::from_str::<ThemeFile>(SHARED_THEME).unwrap();
        let theme = Theme::from_file(&file).unwrap();

        assert_eq!(theme.background.bg, Some(Color::Rgb(0x22, 0x24, 0x36)));
        assert_eq!(theme.border_selected.fg, Some(Color::Rgb(0xc0, 0x99, 0xff)));
        assert_eq!(theme.title.fg, Some(Color::Rgb(0x82, 0xaa, 0xff)));
        assert_eq!(theme.mode_search.bg, Some(Color::Rgb(0x4f, 0xd6, 0xbe)));
        assert_eq!(theme.search_input.bg, Some(Color::Rgb(0x2f, 0x33, 0x4d)));
    }

    #[test]
    fn catalog_is_explicit_and_loading_does_not_rewrite_config() {
        let root = test_root();
        let home = root.path();
        let themes_dir = home.join("themes");
        std::fs::create_dir_all(&themes_dir).unwrap();
        let selected = themes_dir.join("synthetic-theme.toml");
        std::fs::write(&selected, SHARED_THEME).unwrap();
        std::fs::write(themes_dir.join("unlisted.toml"), SHARED_THEME).unwrap();
        let catalog = home.join("catalog.toml");
        std::fs::write(&catalog, format!("themes = [\"{}\"]\n", selected.display())).unwrap();
        let config = format!(
            "theme = \"{}\"\ntheme_catalog = \"{}\"\n",
            selected.display(),
            catalog.display()
        );
        let config_path = write_config(home, &config);

        let set = load_themes_at(home, None).unwrap();

        assert_eq!(set.themes.len(), 1);
        assert_eq!(set.themes[0].name, "synthetic theme");
        assert_eq!(set.selected, 0);
        assert_eq!(std::fs::read_to_string(config_path).unwrap(), config);
    }

    #[test]
    fn no_config_falls_back_to_the_built_in_theme() {
        let root = test_root();
        let set = load_themes_at(root.path(), None).unwrap();
        assert_eq!(set.themes.len(), 1);
        assert_eq!(set.themes[0].name, "tokyo night moon fallback");
        assert!(set.themes[0].path.is_none());
        assert_eq!(set.selected, 0);
        assert_eq!(set.themes[0].theme.title, Theme::default().title);
    }

    #[test]
    fn cli_theme_selects_a_catalog_entry_by_normalized_name() {
        let root = test_root();
        let home = root.path();
        write_catalog(
            home,
            &[
                "~/themes/tokyo-night.toml",
                "~/themes/catppuccin-mocha.toml",
            ],
        );

        let set = load_themes_at(home, Some("Tokyo_Night")).unwrap();
        assert_eq!(set.themes.len(), 2);
        assert_eq!(set.selected, 0);

        let set = load_themes_at(home, Some("catppuccin")).unwrap();
        assert_eq!(set.themes[set.selected].name, "catppuccin mocha");

        // Without a theme setting, the first catalog entry wins.
        let set = load_themes_at(home, None).unwrap();
        assert_eq!(set.selected, 0);
    }

    #[test]
    fn duplicate_and_unreadable_catalog_entries_are_skipped() {
        let root = test_root();
        let home = root.path();
        let catalog = write_catalog(home, &["~/themes/one.toml", "~/themes/one.toml"]);
        let catalog_text = std::fs::read_to_string(&catalog).unwrap().replace(
            ']',
            ", \"~/themes/missing.toml\", \"~/themes/broken.toml\"]",
        );
        std::fs::write(&catalog, catalog_text).unwrap();
        std::fs::write(
            home.join("themes/broken.toml"),
            "[colors]\nbg = \"#222436\"\n",
        )
        .unwrap();

        let set = load_themes_at(home, None).unwrap();
        assert_eq!(set.themes.len(), 1);
        assert_eq!(set.themes[0].name, "one");
    }

    #[test]
    fn cli_theme_loads_a_file_outside_the_catalog() {
        let root = test_root();
        let home = root.path();
        write_catalog(home, &["~/themes/listed.toml"]);
        std::fs::write(home.join("extra-theme.toml"), SHARED_THEME).unwrap();

        let set = load_themes_at(home, Some("~/extra-theme.toml")).unwrap();
        assert_eq!(set.themes.len(), 2);
        assert_eq!(set.selected, 1);
        assert_eq!(set.themes[1].name, "extra theme");

        // A catalog file as the theme selects the entry and loads nothing twice.
        let set = load_themes_at(home, Some("~/themes/listed.toml")).unwrap();
        assert_eq!(set.themes.len(), 1);
        assert_eq!(set.selected, 0);
    }

    #[test]
    fn unknown_cli_theme_without_a_catalog_is_an_error() {
        let root = test_root();
        let home = root.path();

        let error = load_themes_at(home, Some("nope")).err().unwrap();
        assert_eq!(
            error,
            "theme 'nope' is not a readable file or configured catalog entry"
        );

        // A broken theme file named on the command line is an error too.
        let broken = home.join("broken.toml");
        std::fs::write(&broken, "not toml [").unwrap();
        assert!(load_themes_at(home, Some(broken.to_str().unwrap())).is_err());
    }

    #[test]
    fn unknown_configured_theme_falls_back_to_the_first_entry() {
        let root = test_root();
        let home = root.path();
        write_catalog(home, &["~/themes/alpha.toml"]);
        let config_path = home.join(".config/viewr/config.toml");
        let config = std::fs::read_to_string(&config_path).unwrap();
        std::fs::write(&config_path, format!("theme = \"nope\"\n{config}")).unwrap();

        let set = load_themes_at(home, None).unwrap();
        assert_eq!(set.selected, 0);
        assert_eq!(set.themes[0].name, "alpha");
    }

    #[test]
    fn missing_or_broken_catalog_gives_no_entries() {
        let root = test_root();
        let home = root.path();
        assert!(load_catalog_paths(&home.join("missing.toml"), home).is_empty());
        let broken = home.join("broken.toml");
        std::fs::write(&broken, "themes = 7").unwrap();
        assert!(load_catalog_paths(&broken, home).is_empty());
    }

    #[test]
    fn minimal_theme_fills_the_roles_from_bg_and_fg() {
        let file = toml::from_str::<ThemeFile>(
            "[colors]\nbg = \"#101010\"\nfg = \"#f0f0f0\"\n[ui]\naccent = \"#ABCDEF\"\n",
        )
        .unwrap();
        let theme = Theme::from_file(&file).unwrap();
        assert_eq!(theme.status_bar.bg, Some(Color::Rgb(0x10, 0x10, 0x10)));
        assert_eq!(theme.border.fg, Some(Color::Rgb(0xf0, 0xf0, 0xf0)));
        assert_eq!(theme.border_selected.fg, Some(Color::Rgb(0xab, 0xcd, 0xef)));
        assert_eq!(theme.popup_title.fg, Some(Color::Rgb(0xab, 0xcd, 0xef)));
    }

    #[test]
    fn parse_hex_needs_a_hash_and_six_hex_digits() {
        assert_eq!(parse_hex("#0a0B0c"), Some(Color::Rgb(10, 11, 12)));
        assert_eq!(parse_hex("0a0b0c"), None);
        assert_eq!(parse_hex("#abc"), None);
        assert_eq!(parse_hex("#gg0000"), None);
        assert_eq!(parse_hex("#ä0000"), None);
    }

    #[test]
    fn names_come_from_the_file_stem() {
        assert_eq!(
            theme_name(Path::new("/x/rose-pine-moon.toml")),
            "rose pine moon"
        );
        assert_eq!(normalized_name("Rosé Pine-Moon"), "rospinemoon");
        assert_eq!(
            expand_home(Path::new("/home/synthetic"), "~/themes/a.toml"),
            PathBuf::from("/home/synthetic/themes/a.toml")
        );
        assert_eq!(
            expand_home(Path::new("/h"), "/abs.toml"),
            PathBuf::from("/abs.toml")
        );
    }

    fn test_root() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    /// Write `config.toml` below `home` and return its path.
    fn write_config(home: &Path, contents: &str) -> PathBuf {
        let config_dir = home.join(".config/viewr");
        std::fs::create_dir_all(&config_dir).unwrap();
        let config_path = config_dir.join("config.toml");
        std::fs::write(&config_path, contents).unwrap();
        config_path
    }

    /// Write a catalog with `entries`, the theme files they name, and a config that uses it.
    fn write_catalog(home: &Path, entries: &[&str]) -> PathBuf {
        for entry in entries {
            let path = expand_home(home, entry);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, SHARED_THEME).unwrap();
        }
        let list = entries
            .iter()
            .map(|entry| format!("\"{entry}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let catalog = home.join("catalog.toml");
        std::fs::write(&catalog, format!("themes = [{list}]\n")).unwrap();
        write_config(home, "theme_catalog = \"~/catalog.toml\"\n");
        catalog
    }
}
