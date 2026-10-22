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
    /// True when the session captured the mouse at the start. The restore releases it, also
    /// when the run loop released it in between: a second release changes nothing.
    mouse: bool,
}

impl TerminalSession {
    /// Enter raw mode and the alternate screen. With `mouse`, the terminal reports clicks, drags
    /// and the wheel as events. Without it, the terminal keeps its own selection and scrolling.
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
        let mut first_error = encoder::KittyBackend::default()
            .delete_all_to(backend)
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

/// The mouse modes that viewr needs: 1000 (press and release), 1002 (motion only while a button
/// is held, for the drag) and 1006 (SGR coordinates, which crossterm parses). viewr leaves out
/// 1003 (any motion), so a plain pointer move does not wake the run loop.
const MOUSE_ON: &[u8] = b"\x1b[?1000h\x1b[?1002h\x1b[?1006h";

/// The same modes off, in the reverse order.
const MOUSE_OFF: &[u8] = b"\x1b[?1006l\x1b[?1002l\x1b[?1000l";

/// Let the terminal report clicks, drags and the wheel, but not a plain pointer move.
pub(crate) fn enable_mouse(out: &mut impl Write) -> io::Result<()> {
    out.write_all(MOUSE_ON)?;
    out.flush()
}

/// Give the mouse back to the terminal: its own selection and scrolling.
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
    query_terminal_until(sequence, |answer| {
        answer
            .windows(2)
            .position(|pair| pair == b"\x1b\\")
            .map(|end| end + 2)
    })
}

/// Send `sequence` to the terminal and read until `end` finds the length of the complete
/// answer, or `None` after `PROBE_TIMEOUT`.
///
/// The read takes every byte that the tty holds, so a key pressed before the answer is lost.
/// The caller asks only when the user does not type: at start, and after a resize when the
/// terminal answered at start, so no timeout can block the loop.
fn query_terminal_until(sequence: &[u8], end: impl Fn(&[u8]) -> Option<usize>) -> Option<Vec<u8>> {
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
        if let Some(length) = end(&answer) {
            answer.truncate(length);
            return Some(answer);
        }
    }
}

/// The pixel size of one cell, as the terminal answers `CSI 16 t`. `None` when the terminal
/// does not answer in time, or with something else.
///
/// The window size (TIOCGWINSZ) includes the window padding of Ghostty, so its division gives
/// a cell that is a fraction too tall, and a grid of 50 rows drifts by a cell. The terminal
/// answers the query with its real cell size.
pub(crate) fn query_cell_pixel_size() -> Option<(u32, u32)> {
    query_terminal_until(CELL_SIZE_QUERY, cell_size_report_end)
        .as_deref()
        .and_then(parse_cell_size_report)
}

/// The cell size from the window size divided by the cell count, when the terminal does not
/// answer the query.
pub(crate) fn window_cell_pixel_size() -> (u32, u32) {
    crossterm::terminal::window_size().map_or((8, 16), |ws| cell_pixel_size(&ws))
}

/// `CSI 16 t` asks for the pixel size of a cell. The answer is `CSI 6 ; height ; width t`.
const CELL_SIZE_QUERY: &[u8] = b"\x1b[16t";

/// The length of the answer to `CELL_SIZE_QUERY` in `answer`, once its final `t` is there.
fn cell_size_report_end(answer: &[u8]) -> Option<usize> {
    let start = answer.windows(3).position(|bytes| bytes == b"\x1b[6")?;
    let length = answer[start..].iter().position(|&byte| byte == b't')?;
    Some(start + length + 1)
}

/// The cell size in `CSI 6 ; height ; width t`. `None` for any other answer or a zero size.
fn parse_cell_size_report(answer: &[u8]) -> Option<(u32, u32)> {
    let text = std::str::from_utf8(answer).ok()?;
    let (_, report) = text.split_once("\x1b[6;")?;
    let (fields, _) = report.split_once('t')?;
    let (height, width) = fields.split_once(';')?;
    let (width, height) = (width.parse::<u32>().ok()?, height.parse::<u32>().ok()?);
    (width > 0 && height > 0).then_some((width, height))
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
    use super::{
        CELL_SIZE_QUERY, cell_pixel_size, cell_size_report_end, disable_mouse, enable_mouse,
        parse_cell_size_report,
    };
    use crossterm::terminal::WindowSize;

    #[test]
    fn the_mouse_modes_are_press_drag_and_sgr_without_any_motion() {
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

    #[test]
    fn the_cell_size_query_and_its_answer_follow_xterm() {
        assert_eq!(CELL_SIZE_QUERY, b"\x1b[16t");
        // Ghostty and herdr answer 18x43 px cells like this.
        assert_eq!(parse_cell_size_report(b"\x1b[6;43;18t"), Some((18, 43)));
        assert_eq!(
            parse_cell_size_report(b"\x1b[I\x1b[6;43;18t"),
            Some((18, 43)),
            "a focus report before the answer"
        );
    }

    #[test]
    fn a_strange_or_empty_answer_gives_no_cell_size() {
        assert_eq!(parse_cell_size_report(b""), None);
        assert_eq!(parse_cell_size_report(b"\x1b[0n"), None);
        assert_eq!(parse_cell_size_report(b"\x1b[6;0;18t"), None);
        assert_eq!(parse_cell_size_report(b"\x1b[6;43t"), None);
        assert_eq!(parse_cell_size_report(b"\x1b[6;x;18t"), None);
    }

    #[test]
    fn the_read_stops_at_the_final_t_of_the_report() {
        assert_eq!(cell_size_report_end(b"\x1b[6;43;18t"), Some(10));
        assert_eq!(cell_size_report_end(b"\x1b[6;43;18tj"), Some(10));
        assert_eq!(cell_size_report_end(b"\x1b[6;43;1"), None);
        assert_eq!(cell_size_report_end(b"tt"), None);
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
