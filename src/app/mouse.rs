//! The mouse state: the last click for the double-click, the drag in the zoomed view, and the
//! screen areas of the last draw that a click lands on.

use ratatui::layout::{Position, Rect};
use std::time::{Duration, Instant};

/// Two clicks on the same target within this time are a double-click.
pub(crate) const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// The areas of the last draw that the mouse handler needs and that the view state does not
/// hold otherwise. `ui::draw` sets them each frame. They are empty until the first draw.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScreenLayout {
    /// The whole terminal.
    pub(crate) screen: Rect,
}

/// The click before the current one, for the double-click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LastClick {
    pub(crate) at: Instant,
    /// The cell position in the gallery.
    pub(crate) target: usize,
}

/// A left button that went down in the zoomed fullscreen view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Drag {
    /// The last position that the pan used.
    pub(crate) last: Position,
    /// True once the mouse moved: the release is then no click.
    pub(crate) moved: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MouseState {
    pub(crate) last_click: Option<LastClick>,
    pub(crate) drag: Option<Drag>,
}

impl MouseState {
    /// True when `target` got a click within `DOUBLE_CLICK` before `now`. The click is
    /// remembered either way, so the third click starts a new pair.
    pub(crate) fn double_click(&mut self, target: usize, now: Instant) -> bool {
        let double = self.last_click.take().is_some_and(|last| {
            last.target == target && now.saturating_duration_since(last.at) <= DOUBLE_CLICK
        });
        if !double {
            self.last_click = Some(LastClick { at: now, target });
        }
        double
    }
}

#[cfg(test)]
mod tests {
    use super::{DOUBLE_CLICK, MouseState};
    use std::time::{Duration, Instant};

    #[test]
    fn two_clicks_on_one_target_within_the_time_are_a_double_click() {
        let mut mouse = MouseState::default();
        let start = Instant::now();
        assert!(!mouse.double_click(3, start));
        assert!(mouse.double_click(3, start + DOUBLE_CLICK));
        assert!(
            !mouse.double_click(3, start + DOUBLE_CLICK + Duration::from_millis(1)),
            "the third click starts a new pair"
        );
        assert!(
            !mouse.double_click(4, start + DOUBLE_CLICK + Duration::from_millis(2)),
            "another target"
        );
        assert!(
            !mouse.double_click(4, start + Duration::from_secs(2)),
            "too late"
        );
    }
}
