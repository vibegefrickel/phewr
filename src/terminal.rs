//! The terminal session: raw mode, the alternate screen and the mouse modes.

use crate::pictures::kitty;
use crossterm::{
    cursor, execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::prelude::*;
use std::io::{self, Write, stdout};

pub(crate) struct TerminalSession {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    restored: bool,
    /// True when the session captured the mouse at the start. A second release changes nothing.
    mouse: bool,
}

impl TerminalSession {
    /// Enter raw mode and the alternate screen. With `mouse`, the terminal reports clicks, drags
    /// and the wheel.
    pub(crate) fn enter(mouse: bool) -> io::Result<Self> {
        enable_raw_mode()?;
        let mut output = stdout();
        if let Err(error) = execute!(output, EnterAlternateScreen, cursor::Hide) {
            leave_terminal(&mut output, false);
            return Err(error);
        }
        if mouse && let Err(error) = enable_mouse(&mut output) {
            leave_terminal(&mut output, true);
            return Err(error);
        }

        match Terminal::new(CrosstermBackend::new(output)) {
            Ok(terminal) => Ok(Self {
                terminal,
                restored: false,
                mouse,
            }),
            Err(error) => {
                leave_terminal(&mut stdout(), mouse);
                Err(error)
            }
        }
    }

    pub(crate) fn terminal_mut(&mut self) -> &mut Terminal<CrosstermBackend<io::Stdout>> {
        &mut self.terminal
    }

    pub(crate) fn restore(&mut self) -> io::Result<()> {
        if self.restored {
            return Ok(());
        }
        self.restored = true;

        let backend = self.terminal.backend_mut();
        let mut first_error = kitty::protocol::delete_all_to(backend)
            .and_then(|()| Write::flush(backend))
            .err();
        if self.mouse
            && let Err(error) = disable_mouse(self.terminal.backend_mut())
            && first_error.is_none()
        {
            first_error = Some(error);
        }
        if let Err(error) = disable_raw_mode()
            && first_error.is_none()
        {
            first_error = Some(error);
        }
        if let Err(error) = execute!(
            self.terminal.backend_mut(),
            cursor::Show,
            LeaveAlternateScreen
        ) && first_error.is_none()
        {
            first_error = Some(error);
        }

        first_error.map_or(Ok(()), Err)
    }
}

/// 1000 (press and release), 1002 (motion while a button is held) and 1006 (SGR coordinates).
/// Without 1003 (any motion), a plain pointer move does not wake the run loop.
const MOUSE_ON: &[u8] = b"\x1b[?1000h\x1b[?1002h\x1b[?1006h";

/// The same modes off, in the reverse order.
const MOUSE_OFF: &[u8] = b"\x1b[?1006l\x1b[?1002l\x1b[?1000l";

pub(crate) fn enable_mouse(out: &mut impl Write) -> io::Result<()> {
    out.write_all(MOUSE_ON)?;
    out.flush()
}

pub(crate) fn disable_mouse(out: &mut impl Write) -> io::Result<()> {
    out.write_all(MOUSE_OFF)?;
    out.flush()
}

/// Undo raw mode and the alternate screen after a failed start.
#[expect(
    clippy::let_underscore_must_use,
    reason = "the start error matters more than a failed cleanup"
)]
fn leave_terminal(output: &mut io::Stdout, mouse: bool) {
    if mouse {
        let _ = disable_mouse(output);
    }
    let _ = disable_raw_mode();
    let _ = execute!(output, cursor::Show, LeaveAlternateScreen);
}

impl Drop for TerminalSession {
    #[expect(
        clippy::let_underscore_must_use,
        reason = "restore is best effort during a panic or an early return"
    )]
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

#[cfg(test)]
mod tests {
    use super::{disable_mouse, enable_mouse};

    #[test]
    fn mouse_modes_skip_motion() {
        let mut on = Vec::new();
        enable_mouse(&mut on).unwrap();
        assert_eq!(on, b"\x1b[?1000h\x1b[?1002h\x1b[?1006h");
        let mut off = Vec::new();
        disable_mouse(&mut off).unwrap();
        assert_eq!(off, b"\x1b[?1006l\x1b[?1002l\x1b[?1000l");
        for out in [&on, &off] {
            assert!(!String::from_utf8_lossy(out).contains("1003"), "{out:?}");
        }
    }
}
