//! The terminal session: raw mode, the alternate screen, the Kitty probe and the cell size.

use crate::encoder::{self, GraphicsBackend};
use crossterm::{
    cursor, execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::prelude::*;
use std::io::{self, Write, stdout};
use std::time::{Duration, Instant};

pub(crate) struct TerminalSession {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    restored: bool,
}

impl TerminalSession {
    pub(crate) fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut output = stdout();
        if let Err(error) = execute!(output, EnterAlternateScreen, cursor::Hide) {
            leave_terminal(&mut output);
            return Err(error);
        }

        match Terminal::new(CrosstermBackend::new(output)) {
            Ok(terminal) => Ok(Self {
                terminal,
                restored: false,
            }),
            Err(error) => {
                leave_terminal(&mut stdout());
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
        let mut first_error = encoder::KittyBackend::default()
            .delete_all_to(backend)
            .and_then(|()| Write::flush(backend))
            .err();
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

/// Undo raw mode and the alternate screen after a failed start.
#[expect(
    clippy::let_underscore_must_use,
    reason = "the start error matters more than a failed cleanup"
)]
fn leave_terminal(output: &mut io::Stdout) {
    let _ = disable_raw_mode();
    let _ = execute!(output, cursor::Show, LeaveAlternateScreen);
}

impl Drop for TerminalSession {
    #[expect(
        clippy::let_underscore_must_use,
        reason = "a panic or an early return is already on its way; restore is best effort"
    )]
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

/// How long the start waits for the answer of the terminal to the Kitty probe.
const PROBE_TIMEOUT: Duration = Duration::from_millis(300);

/// Send `sequence` to the terminal and return its answer, or `None` after `PROBE_TIMEOUT`.
///
/// The answer must not reach crossterm, which would read it as key presses. The function runs
/// before the first `event::poll`, reads the tty itself and stops at the string terminator.
pub(crate) fn query_terminal(sequence: &[u8]) -> Option<Vec<u8>> {
    use rustix::event::{PollFd, PollFlags};
    use std::io::Read;

    let mut tty = std::fs::File::options()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .ok()?;
    tty.write_all(sequence).ok()?;
    tty.flush().ok()?;

    let deadline = Instant::now() + PROBE_TIMEOUT;
    let mut answer = Vec::new();
    loop {
        let remaining = deadline.checked_duration_since(Instant::now())?;
        let timeout = rustix::event::Timespec::try_from(remaining).ok()?;
        let mut fds = [PollFd::new(&tty, PollFlags::IN)];
        if rustix::event::poll(&mut fds, Some(&timeout)).ok()? == 0 {
            return None;
        }
        let mut buf = [0_u8; 256];
        let count = tty.read(&mut buf).ok()?;
        if count == 0 {
            return None;
        }
        answer.extend_from_slice(&buf[..count]);
        if let Some(end) = answer.windows(2).position(|pair| pair == b"\x1b\\") {
            answer.truncate(end + 2);
            return Some(answer);
        }
    }
}

pub(crate) fn query_cell_pixel_size() -> (u32, u32) {
    crossterm::terminal::window_size().map_or((8, 16), |ws| cell_pixel_size(&ws))
}

/// The pixel size of one cell from a window size report, so that the tests need no terminal.
fn cell_pixel_size(ws: &crossterm::terminal::WindowSize) -> (u32, u32) {
    // A terminal that does not report pixel dimensions (common under
    // terminal multiplexers such as rift/tmux, and some SSH setups)
    // returns ws.width/ws.height == 0 even though columns/rows are
    // valid. Treat a zero pixel report as "unknown" and fall back to
    // conventional cell metrics, otherwise the division below collapses
    // to 1x1 and thumbnails render only a few pixels tall.
    let w = if ws.columns > 0 && ws.width > 0 {
        u32::from(ws.width) / u32::from(ws.columns)
    } else {
        8
    };
    let h = if ws.rows > 0 && ws.height > 0 {
        u32::from(ws.height) / u32::from(ws.rows)
    } else {
        16
    };
    (w.max(1), h.max(1))
}

#[cfg(test)]
mod tests {
    use super::{cell_pixel_size, query_cell_pixel_size};
    use crossterm::terminal::WindowSize;

    #[test]
    fn the_cell_size_is_never_zero() {
        let (width, height) = query_cell_pixel_size();

        assert!(width >= 1 && height >= 1);
    }

    fn window(columns: u16, rows: u16, width: u16, height: u16) -> WindowSize {
        WindowSize {
            rows,
            columns,
            width,
            height,
        }
    }

    #[test]
    fn the_cell_size_divides_the_pixels_by_the_cells() {
        assert_eq!(cell_pixel_size(&window(100, 50, 1000, 1100)), (10, 22));
    }

    #[test]
    fn a_missing_pixel_report_gives_the_default_cell_size() {
        assert_eq!(cell_pixel_size(&window(100, 50, 0, 0)), (8, 16));
        assert_eq!(cell_pixel_size(&window(0, 0, 1000, 1100)), (8, 16));
        assert_eq!(cell_pixel_size(&window(100, 50, 0, 1100)), (8, 22));
    }

    #[test]
    fn a_tiny_pixel_report_gives_at_least_one_pixel() {
        assert_eq!(cell_pixel_size(&window(100, 50, 10, 10)), (1, 1));
    }
}
