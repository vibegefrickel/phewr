//! The styles of the TUI: the terminal default colors and the 16 ANSI colors only. The color
//! scheme of the terminal decides the look.

use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Styles {
    /// The explicit `Reset` keeps the text in a title or a bottom line out of the border color.
    pub text: Style,
    pub title: Style,
    pub border: Style,
    pub popup_border: Style,
    /// The thick border and the label of the cell under the cursor.
    pub cursor_cell: Style,
    /// The plain border, the label and the selection count of a marked image.
    pub marked: Style,
    /// Errors.
    pub error: Style,
    /// Counts, notes and the info panel labels.
    pub dim: Style,
    /// Key hints and the help key column.
    pub key: Style,
}

pub(crate) const STYLES: Styles = Styles {
    text: Style::new().fg(Color::Reset),
    title: Style::new().fg(Color::Reset).add_modifier(Modifier::BOLD),
    border: Style::new().fg(Color::DarkGray),
    popup_border: Style::new(),
    cursor_cell: Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD),
    marked: Style::new().fg(Color::Yellow),
    error: Style::new().fg(Color::Red),
    dim: Style::new().fg(Color::DarkGray),
    key: Style::new().fg(Color::Cyan),
};
