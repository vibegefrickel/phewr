//! The styles of the TUI. viewr uses the default colors of the terminal and the 16 ANSI colors
//! only, so the color scheme of the terminal decides the look.

use ratatui::style::{Color, Modifier, Style};

/// One style per role on the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Styles {
    /// Normal text: the terminal foreground on the terminal background. The explicit `Reset`
    /// keeps the text in a block title or a bottom line out of the dim border color.
    pub text: Style,
    /// Block titles.
    pub title: Style,
    /// The borders of the views.
    pub border: Style,
    /// The border of the help popup, so the popup stands out from the view borders.
    pub popup_border: Style,
    /// The background and the label of the grid cell under the cursor. The background shows
    /// in the gap around the thumbnail.
    pub cursor_cell: Style,
    /// The background and the label of a marked image.
    pub marked: Style,
    /// Errors and the delete question.
    pub error: Style,
    /// The selected row of a picker or a list.
    pub selection: Style,
    /// Secondary text: counts, notes and the labels of the info panel.
    pub dim: Style,
    /// Key hints and the key column of the help popup.
    pub key: Style,
}

/// The styles that every view draws with.
pub(crate) const STYLES: Styles = Styles {
    text: Style::new().fg(Color::Reset),
    title: Style::new().fg(Color::Reset).add_modifier(Modifier::BOLD),
    border: Style::new().fg(Color::DarkGray),
    popup_border: Style::new(),
    cursor_cell: Style::new()
        .fg(Color::White)
        .bg(Color::Blue)
        .add_modifier(Modifier::BOLD),
    marked: Style::new().fg(Color::Black).bg(Color::Yellow),
    error: Style::new().fg(Color::Red),
    selection: Style::new().add_modifier(Modifier::REVERSED),
    dim: Style::new().fg(Color::DarkGray),
    key: Style::new().fg(Color::Cyan),
};
