//! The mouse state: the last click, the drag in the zoomed view and the screen areas of the
//! last draw.

use ratatui::layout::{Position, Rect};
use tuilibr::frame::DoubleClick;

/// The areas of the last draw that the mouse handler needs. `ui::draw` sets them each frame.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScreenLayout {
    pub(crate) screen: Rect,
}

/// A left button that went down in the zoomed fullscreen view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Drag {
    /// The last position that the pan used.
    pub(crate) last: Position,
    /// True once the mouse moved: the release is then no click.
    pub(crate) moved: bool,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct MouseState {
    /// The last click on a cell position in the gallery.
    pub(crate) clicks: DoubleClick<usize>,
    pub(crate) drag: Option<Drag>,
}
