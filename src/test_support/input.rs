//! Key and mouse input through the real event dispatch.

use crate::app::App;
use crate::keys::{handle_event, handle_event_at};
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use std::time::Instant;

pub(crate) fn press(app: &mut App, code: KeyCode) -> bool {
    press_with(app, code, KeyModifiers::NONE)
}

pub(crate) fn press_with(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> bool {
    handle_event(app, &Event::Key(KeyEvent::new(code, modifiers))).unwrap()
}

pub(crate) fn char_key(app: &mut App, c: char) -> bool {
    press(app, KeyCode::Char(c))
}

/// A mouse event at `(column, row)`, with `now` as the double-click clock.
pub(crate) fn mouse_at(
    app: &mut App,
    kind: MouseEventKind,
    (column, row): (u16, u16),
    modifiers: KeyModifiers,
    now: Instant,
) -> bool {
    let event = Event::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers,
    });
    handle_event_at(app, &event, now).unwrap()
}

pub(crate) fn mouse(app: &mut App, kind: MouseEventKind, at: (u16, u16)) -> bool {
    mouse_at(app, kind, at, KeyModifiers::NONE, Instant::now())
}

pub(crate) fn click_at(app: &mut App, at: (u16, u16), now: Instant) -> bool {
    mouse_at(
        app,
        MouseEventKind::Down(MouseButton::Left),
        at,
        KeyModifiers::NONE,
        now,
    ) || mouse_at(
        app,
        MouseEventKind::Up(MouseButton::Left),
        at,
        KeyModifiers::NONE,
        now,
    )
}

pub(crate) fn click(app: &mut App, at: (u16, u16)) -> bool {
    click_at(app, at, Instant::now())
}

pub(crate) fn cell_center(app: &App, vis: usize) -> (u16, u16) {
    let cell = app.gallery.thumb_rect(vis);
    (cell.x + cell.width / 2, cell.y + cell.height / 2)
}

pub(crate) fn click_cell(app: &mut App, vis: usize, now: Instant) -> bool {
    let at = cell_center(app, vis);
    click_at(app, at, now)
}

pub(crate) fn mouse_cell(app: &mut App, kind: MouseEventKind, vis: usize) -> bool {
    let at = cell_center(app, vis);
    mouse(app, kind, at)
}

/// Draw one frame into a test backend. The app records the screen layout.
pub(crate) fn draw_frame(app: &mut App, width: u16, height: u16) {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| crate::ui::draw(frame, app, None))
        .unwrap();
}
