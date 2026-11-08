//! The terminal session: raw mode, the alternate screen, the Kitty probe and the cell size.

use crate::encoder;
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
        let mut first_error = encoder::kitty::delete_all_to(backend)
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

const PROBE_TIMEOUT: Duration = Duration::from_millis(300);

/// The answers at the start. `None` for an answer that did not come in time.
pub(crate) struct StartAnswers {
    pub(crate) cell_px: Option<(u32, u32)>,
    /// The bytes up to the end of the last Kitty answer that came in time.
    pub(crate) probe: Option<Vec<u8>>,
    /// The owed Kitty answers that can still come late, as key presses. Zero when a later device
    /// attributes answer came.
    pub(crate) late_answers: usize,
    /// The foreground color of the terminal (OSC 10), the color of the grid icons.
    pub(crate) foreground: Option<[u8; 3]>,
}

/// The primary device attributes query (DA1). Every terminal answers it after the queries
/// before it.
const DEVICE_ATTRIBUTES_QUERY: &[u8] = b"\x1b[c";

/// OSC 10 asks for the foreground color. The answer ends with BEL like the query, so it never
/// counts as a Kitty answer.
const FOREGROUND_QUERY: &[u8] = b"\x1b]10;?\x07";

/// Send the cell size query, the foreground query, the Kitty `probe` and DA1 in one write, and wait for the cell size
/// and for `answers` Kitty answers.
///
/// A terminal answers in order. The DA1 answer ends the wait even when the terminal skips a
/// Kitty answer. Only a terminal that answers neither costs `PROBE_TIMEOUT`.
///
/// The answers must not reach crossterm. It reads them as key presses. The function runs
/// before the first `event::poll` and reads the tty itself.
pub(crate) fn query_cell_size_and_probe(probe: &[u8], answers: usize) -> StartAnswers {
    let sequence = start_sequence(probe);
    let mut answer = Vec::new();
    // After a timeout, `answer` holds the answers that came in time.
    query_terminal_into(&sequence, &mut answer, |answer| {
        start_answers_end(answer, answers)
    });
    start_answers(&answer, answers)
}

/// The cell size query, the foreground query, the Kitty `probe` and DA1, in the order of the
/// answers. The foreground answer comes before the Kitty answers, so the read waits for it
/// without an end condition of its own.
fn start_sequence(probe: &[u8]) -> Vec<u8> {
    let mut sequence = CELL_SIZE_QUERY.to_vec();
    sequence.extend_from_slice(FOREGROUND_QUERY);
    sequence.extend_from_slice(probe);
    sequence.extend_from_slice(DEVICE_ATTRIBUTES_QUERY);
    sequence
}

/// The length of the start answers once all are complete, in any order. The DA1 answer ends
/// them early.
fn start_answers_end(answer: &[u8], answers: usize) -> Option<usize> {
    if let Some(attributes) = device_attributes_end(answer) {
        let others = [
            cell_size_report_end(answer),
            string_terminator_ends(answer).last(),
        ];
        return Some(others.into_iter().flatten().fold(attributes, usize::max));
    }
    Some(cell_size_report_end(answer)?.max(string_terminators_end(answer, answers)?))
}

fn start_answers(answer: &[u8], answers: usize) -> StartAnswers {
    let received = string_terminator_ends(answer).count();
    let last_end = string_terminator_ends(answer).last();
    let probe_complete = string_terminators_end(answer, answers).is_some()
        || device_attributes_end(answer).is_some();
    let foreground = parse_foreground(answer);
    StartAnswers {
        cell_px: parse_cell_size_report(answer),
        foreground,
        probe: last_end.map(|end| answer[..end].to_vec()),
        // A missing OSC 10 answer can come late too, before the Kitty answers.
        late_answers: if probe_complete {
            0
        } else {
            answers.saturating_sub(received) + usize::from(foreground.is_none())
        },
    }
}

/// The end of the DA1 answer, `CSI ? <numbers> c`.
fn device_attributes_end(answer: &[u8]) -> Option<usize> {
    answer.windows(3).enumerate().find_map(|(start, head)| {
        if head != b"\x1b[?" {
            return None;
        }
        let body = &answer[start + 3..];
        let length = body
            .iter()
            .position(|&byte| !byte.is_ascii_digit() && byte != b';')?;
        (body[length] == b'c').then_some(start + 3 + length + 1)
    })
}

/// The end of each Kitty answer, `ESC _ ... ESC \`. An OSC answer with the same terminator does
/// not count.
fn string_terminator_ends(answer: &[u8]) -> impl Iterator<Item = usize> + '_ {
    let mut from = 0;
    std::iter::from_fn(move || {
        let start = from + find(&answer[from..], b"\x1b_")?;
        let end = start + find(&answer[start..], b"\x1b\\")? + 2;
        from = end;
        Some(end)
    })
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// The color of the OSC 10 answer, `ESC ] 10 ; rgb:RRRR/GGGG/BBBB` with 1 to 4 hex digits per
/// channel.
fn parse_foreground(answer: &[u8]) -> Option<[u8; 3]> {
    const HEAD: &[u8] = b"\x1b]10;rgb:";
    let start = find(answer, HEAD)? + HEAD.len();
    let body = &answer[start..];
    let end = body
        .iter()
        .position(|byte| !byte.is_ascii_hexdigit() && *byte != b'/')?;
    let mut channels = std::str::from_utf8(&body[..end])
        .ok()?
        .split('/')
        .map(|hex| {
            let digits = u32::try_from(hex.len())
                .ok()
                .filter(|n| (1..=4).contains(n))?;
            let value = u32::from_str_radix(hex, 16).ok()?;
            // Scale 1 to 4 hex digits to one byte.
            u8::try_from(value * 255 / ((1 << (4 * digits)) - 1)).ok()
        });
    let color = [channels.next()??, channels.next()??, channels.next()??];
    channels.next().is_none().then_some(color)
}

/// The length of the first `count` answers with the string terminator, once all are there.
fn string_terminators_end(answer: &[u8], count: usize) -> Option<usize> {
    match count.checked_sub(1) {
        Some(last) => string_terminator_ends(answer).nth(last),
        None => Some(0),
    }
}

fn query_terminal_until(sequence: &[u8], end: impl Fn(&[u8]) -> Option<usize>) -> Option<Vec<u8>> {
    let mut answer = Vec::new();
    query_terminal_into(sequence, &mut answer, end)?;
    Some(answer)
}

/// Send `sequence` to the tty and read the answer into `answer`. `None` after `PROBE_TIMEOUT`.
///
/// The read takes every byte of the tty, and a key typed before the answer is lost. The caller
/// asks only at the start, and after a resize when the terminal answered at the start.
fn query_terminal_into(
    sequence: &[u8],
    answer: &mut Vec<u8>,
    end: impl Fn(&[u8]) -> Option<usize>,
) -> Option<()> {
    let mut tty = std::fs::File::options()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .ok()?;
    tty.write_all(sequence).ok()?;
    tty.flush().ok()?;
    read_answer(&mut tty, answer, Instant::now() + PROBE_TIMEOUT, end)
}

/// Read into `answer` until `end` finds the complete answer. `None` at `deadline` or at the end
/// of the input, and `answer` keeps the bytes that came.
fn read_answer(
    source: &mut (impl io::Read + std::os::fd::AsFd),
    answer: &mut Vec<u8>,
    deadline: Instant,
    end: impl Fn(&[u8]) -> Option<usize>,
) -> Option<()> {
    use rustix::event::{PollFd, PollFlags};

    loop {
        let remaining = deadline.checked_duration_since(Instant::now())?;
        let timeout = rustix::event::Timespec::try_from(remaining).ok()?;
        let mut fds = [PollFd::new(source, PollFlags::IN)];
        if rustix::event::poll(&mut fds, Some(&timeout)).ok()? == 0 {
            return None;
        }
        let mut buf = [0_u8; 256];
        let count = source.read(&mut buf).ok()?;
        if count == 0 {
            return None;
        }
        answer.extend_from_slice(&buf[..count]);
        if let Some(length) = end(answer) {
            answer.truncate(length);
            return Some(());
        }
    }
}

/// The cell size that the terminal answers to `CSI 16 t`.
///
/// The window size (TIOCGWINSZ) includes the Ghostty window padding. Its division gives a cell
/// that is a fraction too tall, and a grid of 50 rows drifts by a cell.
pub(crate) fn query_cell_pixel_size() -> Option<(u32, u32)> {
    query_terminal_until(CELL_SIZE_QUERY, cell_size_report_end)
        .as_deref()
        .and_then(parse_cell_size_report)
}

/// The cell size from the window size, when the terminal does not answer the query.
pub(crate) fn window_cell_pixel_size() -> (u32, u32) {
    crossterm::terminal::window_size().map_or((8, 16), |ws| cell_pixel_size(&ws))
}

/// `CSI 16 t` asks for the cell size. The answer is `CSI 6 ; height ; width t`.
const CELL_SIZE_QUERY: &[u8] = b"\x1b[16t";

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

fn cell_pixel_size(ws: &crossterm::terminal::WindowSize) -> (u32, u32) {
    // tmux and SSH can report 0 pixels with valid columns and rows. A zero report gets the
    // conventional cell metrics, not a 1x1 cell.
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
        CELL_SIZE_QUERY, DEVICE_ATTRIBUTES_QUERY, FOREGROUND_QUERY, cell_pixel_size,
        cell_size_report_end, disable_mouse, enable_mouse, parse_cell_size_report,
        parse_foreground, read_answer, start_answers, start_answers_end, start_sequence,
    };
    use crossterm::terminal::WindowSize;
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};

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

    #[test]
    fn cell_size_query_follows_xterm() {
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
    fn bad_answer_gives_no_cell_size() {
        assert_eq!(parse_cell_size_report(b""), None);
        assert_eq!(parse_cell_size_report(b"\x1b[0n"), None);
        assert_eq!(parse_cell_size_report(b"\x1b[6;0;18t"), None);
        assert_eq!(parse_cell_size_report(b"\x1b[6;43t"), None);
        assert_eq!(parse_cell_size_report(b"\x1b[6;x;18t"), None);
    }

    #[test]
    fn read_stops_at_final_t() {
        assert_eq!(cell_size_report_end(b"\x1b[6;43;18t"), Some(10));
        assert_eq!(cell_size_report_end(b"\x1b[6;43;18tj"), Some(10));
        assert_eq!(cell_size_report_end(b"\x1b[6;43;1"), None);
        assert_eq!(cell_size_report_end(b"tt"), None);
    }

    #[test]
    fn start_read_takes_any_order() {
        let cell = b"\x1b[6;43;18t".as_slice();
        let probe = b"\x1b_Gi=31;OK\x1b\\".as_slice();
        for answer in [[cell, probe].concat(), [probe, cell].concat()] {
            assert_eq!(start_answers_end(&answer, 1), Some(answer.len()));
            let answers = start_answers(&answer, 1);
            assert_eq!(answers.cell_px, Some((18, 43)));
            assert_eq!(answers.late_answers, 0);
            let probe_answer = answers.probe.unwrap();
            assert!(probe_answer.ends_with(probe), "{probe_answer:?}");
        }
        let typed = [cell, probe, b"j"].concat();
        assert_eq!(start_answers_end(&typed, 1), Some(typed.len() - 1));
        assert_eq!(
            start_answers_end(cell, 1),
            None,
            "the probe answer is missing"
        );
        assert_eq!(
            start_answers_end(probe, 1),
            None,
            "the cell size is missing"
        );
    }

    #[test]
    fn start_read_waits_for_kitty_answers() {
        let cell = b"\x1b[6;43;18t".as_slice();
        let transfer = b"\x1b_Gi=31;OK\x1b\\".as_slice();
        let animation = b"\x1b_Gi=32;OK\x1b\\".as_slice();
        let answer = [transfer, cell, animation].concat();
        assert_eq!(start_answers_end(&answer, 2), Some(answer.len()));
        let answers = start_answers(&answer, 2);
        assert_eq!(answers.late_answers, 0);
        assert_eq!(answers.probe.as_deref(), Some(answer.as_slice()));

        let first_only = [cell, transfer].concat();
        assert_eq!(start_answers_end(&first_only, 2), None);
        let answers = start_answers(&first_only, 2);
        assert!(answers.late_answers > 0, "the second answer can still come");
        assert_eq!(answers.probe.as_deref(), Some(first_only.as_slice()));
        assert_eq!(
            start_answers_end(cell, 0),
            Some(cell.len()),
            "no Kitty answer owed"
        );
    }

    #[test]
    fn da1_answer_ends_read() {
        let cell = b"\x1b[6;43;18t".as_slice();
        let transfer = b"\x1b_Gi=31;OK\x1b\\".as_slice();
        let attributes = b"\x1b[?62;22;52c".as_slice();
        // The terminal does not answer the animation frame.
        let answer = [cell, transfer, attributes].concat();
        assert_eq!(start_answers_end(&answer, 2), Some(answer.len()));
        let answers = start_answers(&answer, 2);
        assert_eq!(answers.cell_px, Some((18, 43)));
        assert_eq!(answers.late_answers, 0, "no Kitty answer can come late");
        assert_eq!(
            answers.probe.as_deref(),
            Some([cell, transfer].concat().as_slice())
        );

        let typed = [cell, attributes, b"j"].concat();
        assert_eq!(start_answers_end(&typed, 2), Some(typed.len() - 1));
        assert_eq!(
            start_answers_end(b"\x1b[?62;22", 2),
            None,
            "an answer in two reads"
        );
        assert_eq!(
            start_answers_end(&[cell, transfer].concat(), 2),
            None,
            "the second Kitty answer or the device attributes are missing"
        );
    }

    #[test]
    fn foreground_answer_gives_color() {
        let cell = b"\x1b[6;43;18t".as_slice();
        let probe = b"\x1b_Gi=31;OK\x1b\\".as_slice();
        for color in [
            b"\x1b]10;rgb:ffff/8080/0000\x07".as_slice(),
            b"\x1b]10;rgb:ff/80/00\x1b\\",
            b"\x1b]10;rgb:f/8/0\x07",
        ] {
            let answer = [cell, color, probe].concat();
            assert_eq!(start_answers_end(&answer, 1), Some(answer.len()));
            let answers = start_answers(&answer, 1);
            let foreground = answers.foreground.unwrap();
            assert_eq!(foreground[0], 255);
            assert!((128..=137).contains(&foreground[1]), "{foreground:?}");
            assert_eq!(foreground[2], 0);
            assert_eq!(answers.late_answers, 0);
        }
        assert_eq!(
            start_answers_end(&[cell, b"\x1b]10;rgb:ff/ff/ff\x1b\\"].concat(), 1),
            None,
            "an OSC answer is no Kitty answer"
        );
        assert_eq!(parse_foreground(b"\x1b]10;rgb:ff/ff\x07"), None);
        assert_eq!(parse_foreground(b"\x1b]10;rgb:fffff/0/0\x07"), None);
        assert_eq!(parse_foreground(b"\x1b]11;rgb:ff/ff/ff\x07"), None);
    }

    #[test]
    fn timeout_keeps_early_answer() {
        let only_cell = start_answers(b"\x1b[6;43;18t", 1);
        assert_eq!(only_cell.cell_px, Some((18, 43)));
        assert_eq!(only_cell.probe, None);
        assert!(only_cell.late_answers > 0);
        let only_probe = start_answers(b"\x1b_Gi=31;OK\x1b\\", 1);
        assert_eq!(only_probe.cell_px, None);
        assert_eq!(
            only_probe.probe.as_deref(),
            Some(b"\x1b_Gi=31;OK\x1b\\".as_slice())
        );
        let nothing = start_answers(b"", 1);
        assert_eq!((nothing.cell_px, nothing.probe), (None, None));
        assert_eq!(nothing.late_answers, 2, "the OSC 10 and the Kitty answer");
        let with_color = start_answers(b"\x1b]10;rgb:ff/ff/ff\x07", 1);
        assert_eq!(with_color.late_answers, 1, "only the Kitty answer");
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
    fn cell_size_divides_pixels_by_cells() {
        assert_eq!(cell_pixel_size(&window(100, 50, 1000, 1100)), (10, 22));
    }

    #[test]
    fn missing_report_gives_default_size() {
        assert_eq!(cell_pixel_size(&window(100, 50, 0, 0)), (8, 16));
        assert_eq!(cell_pixel_size(&window(0, 0, 1000, 1100)), (8, 16));
        assert_eq!(cell_pixel_size(&window(100, 50, 0, 1100)), (8, 22));
    }

    #[test]
    fn tiny_report_gives_one_pixel() {
        assert_eq!(cell_pixel_size(&window(100, 50, 10, 10)), (1, 1));
    }

    #[test]
    fn start_sequence_keeps_answer_order() {
        let sequence = start_sequence(b"PROBE");
        let probe_at = CELL_SIZE_QUERY.len() + FOREGROUND_QUERY.len();
        assert!(sequence.starts_with(CELL_SIZE_QUERY));
        assert_eq!(&sequence[CELL_SIZE_QUERY.len()..probe_at], FOREGROUND_QUERY);
        assert_eq!(&sequence[probe_at..probe_at + 5], b"PROBE");
        assert!(sequence.ends_with(DEVICE_ATTRIBUTES_QUERY));
    }

    #[test]
    fn read_stops_at_answer_end() {
        let (mut terminal, mut tty) = UnixStream::pair().unwrap();
        terminal.write_all(b"\x1b[6;16;8tkey").unwrap();
        let mut answer = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(2);
        let read = read_answer(&mut tty, &mut answer, deadline, cell_size_report_end);
        assert_eq!(read, Some(()));
        assert_eq!(answer, b"\x1b[6;16;8t");
    }

    #[test]
    fn read_keeps_partial_answer() {
        let (mut terminal, mut tty) = UnixStream::pair().unwrap();
        terminal.write_all(b"\x1b[6;16").unwrap();
        drop(terminal);
        let mut answer = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(2);
        let read = read_answer(&mut tty, &mut answer, deadline, cell_size_report_end);
        assert_eq!(read, None);
        assert_eq!(answer, b"\x1b[6;16");
    }

    #[test]
    fn read_gives_up_at_deadline() {
        let (_terminal, mut tty) = UnixStream::pair().unwrap();
        let mut answer = Vec::new();
        let soon = Instant::now() + Duration::from_millis(5);
        assert_eq!(
            read_answer(&mut tty, &mut answer, soon, cell_size_report_end),
            None
        );
        assert!(answer.is_empty());
        let past = Instant::now();
        assert_eq!(
            read_answer(&mut tty, &mut answer, past, cell_size_report_end),
            None
        );
    }
}
