//! The styles of the TUI: the terminal default colors and the 16 ANSI colors only. The color
//! scheme of the terminal decides the look.

use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Styles {
    /// The explicit `Reset` keeps the text in a title or a bottom line out of the border color.
    pub text: Style,
    pub title: Style,
    /// The plain border of a marked file, and the selection count.
    pub marked: Style,
    /// Errors.
    pub error: Style,
    /// Counts, notes and the info pane labels.
    pub dim: Style,
    /// The slideshow badge on top of the picture. The background hides the picture below it.
    pub badge: Style,
    /// An error in the slideshow badge.
    pub badge_error: Style,
    /// The colors of the mode blocks in the status row. The block shows them reversed.
    pub mode_normal: Color,
    pub mode_fullscreen: Color,
    pub mode_search: Color,
    pub mode_help: Color,
}

pub(crate) const STYLES: Styles = Styles {
    text: Style::new().fg(Color::Reset),
    title: Style::new().fg(Color::Reset).add_modifier(Modifier::BOLD),
    marked: Style::new().fg(Color::Yellow),
    error: tuilibr::frame::alert(),
    dim: tuilibr::frame::dim(),
    badge: Style::new().fg(Color::Black).bg(Color::Gray),
    badge_error: Style::new().fg(Color::Black).bg(Color::Red),
    mode_normal: Color::Red,
    mode_fullscreen: Color::LightGreen,
    mode_search: Color::Blue,
    mode_help: Color::LightCyan,
};
