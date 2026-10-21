//! Video playback through mpv: inside this terminal with `--vo=kitty`, or in a window of its own.
//!
//! The process start sits behind [`Runner`], so the tests use a fake and never start mpv.

use crate::config::VideoPlayer;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;

/// The status note when the configuration asks for mpv and PATH has none.
pub(crate) const MISSING_NOTE: &str = "mpv not found on PATH: built-in player";

/// The exit code of mpv after a quit by the user in [`Playback::Once`]: `q`, `Q`, `Ctrl+C` and,
/// in a window, the close button. mpv also uses it after a signal.
pub(crate) const USER_QUIT: i32 = 4;

/// The exit code of mpv after `n`: viewr goes on to the next file of the list.
pub(crate) const NEXT_FILE: i32 = 10;

/// The exit code of mpv after `N` (Shift+n): viewr goes back to the previous file of the list.
pub(crate) const PREVIOUS_FILE: i32 = 11;

/// The keys that step through the list from inside mpv, with their exit codes.
///
/// mpv 0.41 binds neither `n` nor `N` by default, so the seek keys (the arrows) and the
/// playlist keys (`<`, `>`, Enter) keep their function.
const STEP_KEYS: [(&str, i32); 2] = [("n", NEXT_FILE), ("N", PREVIOUS_FILE)];

/// How mpv plays the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Playback {
    /// Play the file again and again, until the user quits mpv.
    Loop,
    /// Play the file one time for a running slideshow. Only the end of the file exits with 0,
    /// because each key that quits mpv exits with [`USER_QUIT`].
    Once,
}

/// True when mpv in [`Playback::Once`] played the file to its end, and the user did not quit.
pub(crate) fn reached_the_end(code: Option<i32>) -> bool {
    code == Some(0)
}

/// A step through the list that the user asked for inside mpv.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    Next,
    Previous,
}

/// The step that the exit code of mpv asks for, or `None` for each other exit.
pub(crate) fn step(code: Option<i32>) -> Option<Step> {
    match code {
        Some(NEXT_FILE) => Some(Step::Next),
        Some(PREVIOUS_FILE) => Some(Step::Previous),
        _ => None,
    }
}

/// The player after the PATH lookup. The mpv variants hold the path of the program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Player {
    Terminal(PathBuf),
    Window(PathBuf),
    Builtin,
}

/// Turn the configured player into one that can run; without mpv, use the built-in player.
pub(crate) fn resolve(configured: VideoPlayer, mpv: Option<PathBuf>) -> Player {
    match (configured, mpv) {
        (VideoPlayer::Builtin, _) | (_, None) => Player::Builtin,
        (VideoPlayer::Terminal, Some(mpv)) => Player::Terminal(mpv),
        (VideoPlayer::Window, Some(mpv)) => Player::Window(mpv),
    }
}

/// The terminal size for mpv, in cells and in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Geometry {
    pub cols: u16,
    pub rows: u16,
    pub width: u32,
    pub height: u32,
}

/// The size that mpv must get on the command line, because it cannot read a pixel size of 0.
///
/// A terminal that reports its pixel size gets `None`: mpv reads the size itself and follows
/// a resize. `size` is the report of the terminal: columns, rows, width and height in pixels.
pub(crate) fn fallback_geometry(
    size: (u16, u16, u16, u16),
    cell_px: (u32, u32),
) -> Option<Geometry> {
    let (cols, rows, width, height) = size;
    if width > 0 && height > 0 {
        return None;
    }
    Some(Geometry {
        cols,
        rows,
        width: u32::from(cols) * cell_px.0,
        height: u32::from(rows) * cell_px.1,
    })
}

/// The longest side of the terminal in pixels, from the pixel report or from the cells.
pub(crate) fn longest_side(size: (u16, u16, u16, u16), cell_px: (u32, u32)) -> u32 {
    let (cols, rows, width, height) = size;
    if width > 0 && height > 0 {
        return u32::from(width.max(height));
    }
    (u32::from(cols) * cell_px.0).max(u32::from(rows) * cell_px.1)
}

/// The transfer curve of an HDR video, which the software output of `--vo=kitty` cannot map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hdr {
    /// Hybrid log-gamma (ARIB STD-B67): iPhone videos, also below Dolby Vision profile 8.4.
    Hlg,
    /// Perceptual quantizer (SMPTE ST 2084): HDR10 and Dolby Vision profile 8.1.
    Pq,
}

/// The HDR curve of the best video stream. The container and the codec header tell it, so
/// viewr reads no frame.
pub(crate) fn hdr_of(video: &Path) -> Option<Hdr> {
    use ffmpeg::util::color::TransferCharacteristic;
    let input = ffmpeg::format::input(video).ok()?;
    let stream = input.streams().best(ffmpeg::media::Type::Video)?;
    let context = ffmpeg::codec::context::Context::from_parameters(stream.parameters()).ok()?;
    match context
        .decoder()
        .video()
        .ok()?
        .color_transfer_characteristic()
    {
        TransferCharacteristic::ARIB_STD_B67 => Some(Hdr::Hlg),
        TransferCharacteristic::SMPTE2084 => Some(Hdr::Pq),
        _ => None,
    }
}

/// The edge length of the tone mapping cube: 33 points per channel, as in common `.cube` files.
const LUT_SIZE: u32 = 33;
/// The peak of the HDR picture in nits: the HLG reference display and a common PQ master.
const HDR_PEAK_NITS: f64 = 1000.0;
/// The HDR level in nits that becomes SDR white before the highlight roll-off. 500 nits put
/// the result closest to the SDR export of macOS (`avconvert`) on three iPhone HLG clips.
const SDR_WHITE_NITS: f64 = 500.0;
/// BT.2020 to BT.709 primaries, in linear light.
const BT2020_TO_BT709: [[f64; 3]; 3] = [
    [1.6605, -0.5876, -0.0728],
    [-0.1246, 1.1329, -0.0083],
    [-0.0182, -0.1006, 1.1187],
];

/// Display light in nits for one HDR signal value per channel, BT.2020 primaries.
fn hdr_nits(hdr: Hdr, signal: [f64; 3]) -> [f64; 3] {
    match hdr {
        Hdr::Hlg => {
            // The inverse OETF to scene light, then the OOTF of a 1000 nit display (gamma 1.2).
            let (a, b, c) = (0.178_832_77, 0.284_668_92, 0.559_910_73);
            let scene = signal.map(|value| {
                if value <= 0.5 {
                    value * value / 3.0
                } else {
                    (((value - c) / a).exp() + b) / 12.0
                }
            });
            let luma = 0.2627 * scene[0] + 0.6780 * scene[1] + 0.0593 * scene[2];
            let gain = HDR_PEAK_NITS * luma.max(0.0).powf(0.2);
            scene.map(|value| value * gain)
        }
        Hdr::Pq => {
            let (m1, m2) = (0.159_301_757_812_5, 78.843_75);
            let (c1, c2, c3) = (0.835_937_5, 18.851_562_5, 18.6875);
            signal.map(|value| {
                let power = value.powf(1.0 / m2);
                10_000.0 * ((power - c1).max(0.0) / (c2 - c3 * power)).powf(1.0 / m1)
            })
        }
    }
}

/// One SDR output value per channel (BT.709, gamma 2.4) for one HDR signal value.
///
/// The highlights roll off with an extended Reinhard curve on the luminance, so the hue stays.
/// A color outside BT.709 moves toward its gray of the same luminance, not to a clipped hue.
fn tone_map(hdr: Hdr, signal: [f64; 3]) -> [f64; 3] {
    let nits = hdr_nits(hdr, signal);
    let mut linear = BT2020_TO_BT709
        .map(|row| (row[0] * nits[0] + row[1] * nits[1] + row[2] * nits[2]) / SDR_WHITE_NITS);
    let luma = 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2];
    if luma > 0.0 {
        let peak = HDR_PEAK_NITS / SDR_WHITE_NITS;
        let scale = (1.0 + luma / (peak * peak)) / (1.0 + luma);
        linear = linear.map(|value| value * scale);
        let mapped_luma = luma * scale;
        let low = linear[0].min(linear[1]).min(linear[2]);
        if low < 0.0 {
            let toward_gray = -low / (mapped_luma - low);
            linear = linear.map(|value| value + (mapped_luma - value) * toward_gray);
        }
    }
    // A channel above white (a saturated highlight, or PQ above the peak) scales the whole
    // color down, so that the clip keeps the hue.
    let high = linear[0].max(linear[1]).max(linear[2]);
    if high > 1.0 {
        linear = linear.map(|value| value / high);
    }
    linear.map(|value| value.clamp(0.0, 1.0).powf(1.0 / 2.4))
}

/// The tone mapping cube in the `.cube` text format for the `lut3d` filter.
pub(crate) fn tone_map_cube(hdr: Hdr) -> String {
    let last = f64::from(LUT_SIZE - 1);
    let mut cube = format!("LUT_3D_SIZE {LUT_SIZE}\n");
    // The red index runs fastest in a `.cube` file.
    for blue in 0..LUT_SIZE {
        for green in 0..LUT_SIZE {
            for red in 0..LUT_SIZE {
                let signal = [red, green, blue].map(|index| f64::from(index) / last);
                let [r, g, b] = tone_map(hdr, signal);
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "a write to a String cannot fail"
                )]
                let _ = writeln!(cube, "{r:.6} {g:.6} {b:.6}");
            }
        }
    }
    cube
}

/// The cube file for `hdr` in the first of `dirs` that works. viewr writes it when it is missing.
///
/// A directory fails when viewr cannot write there, or when its path holds a character that
/// the syntax of a filter graph uses. The caller passes a temp directory as the last choice.
/// Without a directory that works, mpv plays without the tone mapping.
pub(crate) fn tone_map_file(dirs: &[&Path], hdr: Hdr) -> Option<PathBuf> {
    dirs.iter().find_map(|dir| tone_map_file_in(dir, hdr))
}

/// The cube file for `hdr` in `dir`, see [`tone_map_file`].
fn tone_map_file_in(dir: &Path, hdr: Hdr) -> Option<PathBuf> {
    let name = match hdr {
        Hdr::Hlg => "tonemap-hlg-v2.cube",
        Hdr::Pq => "tonemap-pq-v2.cube",
    };
    let file = dir.join(name);
    // The filter graph and the option list of mpv read these characters as syntax.
    let safe = file.to_str()?.chars().all(|character| {
        !character.is_control()
            && !matches!(character, '\\' | '\'' | ':' | ',' | ';' | '[' | ']' | '=')
    });
    if !safe {
        return None;
    }
    if !file.is_file() {
        std::fs::create_dir_all(dir).ok()?;
        // Write to a temp file and rename it, so that a second viewr never reads half a cube.
        let partial = dir.join(format!("{name}.{}.partial", std::process::id()));
        std::fs::write(&partial, tone_map_cube(hdr)).ok()?;
        std::fs::rename(&partial, &file).ok()?;
    }
    Some(file)
}

/// The picture path for mpv in the terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Picture {
    /// The longest side of the terminal in pixels, see [`longest_side`].
    pub longest_side: u32,
    /// The tone mapping cube of an HDR video, see [`tone_map_file`].
    pub tone_map: Option<PathBuf>,
}

/// The video filter for `--vo=kitty`, a software output.
///
/// The scale to the terminal comes first, so that the rotation of a portrait video and the
/// color conversion touch a small frame and not a 4K frame. For an HDR video, the cube maps
/// the HDR signal to SDR, and `setparams` marks the result as sRGB, so that the output
/// converts nothing more. Without the cube, the output only converts the color space and
/// clips all highlights: the picture is too bright and its colors are too strong.
fn terminal_filter(picture: &Picture) -> OsString {
    let side = picture.longest_side;
    let scale = format!(
        "scale=w=min(iw\\,{side}):h=min(ih\\,{side}):force_original_aspect_ratio=decrease\
         :force_divisible_by=2:flags=bilinear"
    );
    let graph = match &picture.tone_map {
        None => scale,
        Some(cube) => format!(
            "{scale},format=rgb48le,lut3d=file={}:interp=tetrahedral,format=rgb24,\
             setparams=color_primaries=bt709:color_trc=iec61966-2-1:colorspace=gbr:range=pc",
            cube.display()
        ),
    };
    OsString::from(format!("--vf-pre=lavfi=[{graph}]"))
}

/// mpv in the foreground of this terminal: Kitty graphics, keys from the terminal, no text output.
///
/// mpv draws into the alternate screen of viewr. A screen of its own shows the main screen of
/// the shell for a moment at the start and at the end.
///
/// `--hwdec=auto-copy-safe` decodes on the media engine of the Mac and copies the frames back
/// for the software output. That halves the CPU time per frame. A codec without hardware
/// support decodes in software, as before.
pub(crate) fn terminal_args(
    video: &Path,
    geometry: Option<Geometry>,
    playback: Playback,
    picture: &Picture,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "--vo=kitty",
        "--vo-kitty-alt-screen=no",
        "--profile=sw-fast",
        "--hwdec=auto-copy-safe",
        "--really-quiet",
        "--terminal=yes",
        "--input-terminal=yes",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    args.push(terminal_filter(picture));
    push_playback(&mut args, playback, &["q", "Q"]);
    if let Some(geometry) = geometry {
        args.extend(
            [
                format!("--vo-kitty-cols={}", geometry.cols),
                format!("--vo-kitty-rows={}", geometry.rows),
                format!("--vo-kitty-width={}", geometry.width),
                format!("--vo-kitty-height={}", geometry.height),
            ]
            .map(OsString::from),
        );
    }
    push_file(&mut args, video);
    args
}

/// mpv in a window of its own. `--no-terminal` keeps it away from the keys of viewr.
///
/// The window uses the GPU output of mpv. `--hwdec=auto-safe` gives it the hardware frames
/// without a copy, and the GPU output does its own HDR tone mapping.
pub(crate) fn window_args(video: &Path, playback: Playback) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "--no-terminal",
        "--force-window=immediate",
        "--fs",
        "--native-fs=no",
        "--hwdec=auto-safe",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    push_playback(&mut args, playback, &["q", "Q", "CLOSE_WIN"]);
    push_file(&mut args, video);
    args
}

/// Append the loop options and the key bindings.
///
/// Each playback binds `n` and `N` to an exit with [`NEXT_FILE`] and [`PREVIOUS_FILE`], see
/// [`STEP_KEYS`]. [`Playback::Once`] turns off each option of a user `mpv.conf` that keeps mpv
/// open after the file: `keep-open`, `idle` and `loop-playlist`. It also binds each key in
/// `quit_keys` to an exit with [`USER_QUIT`], because a plain `quit` exits with 0, like the end
/// of the file. `Ctrl+C` quits with that code already. `--input-commands` needs mpv 0.38 or
/// later.
///
/// A key binding cannot catch the quit of the macOS app (Cmd+Q, "Quit mpv" in the app menu).
/// The quit event of the app also ends mpv 0.41 with 0, so a window slideshow continues.
fn push_playback(args: &mut Vec<OsString>, playback: Playback, quit_keys: &[&str]) {
    let mut bindings: Vec<(&str, i32)> = STEP_KEYS.to_vec();
    match playback {
        Playback::Loop => args.push(OsString::from("--loop-file=inf")),
        Playback::Once => {
            args.extend(
                [
                    "--loop-file=no",
                    "--keep-open=no",
                    "--idle=no",
                    "--loop-playlist=no",
                ]
                .map(OsString::from),
            );
            bindings.extend(quit_keys.iter().map(|key| (*key, USER_QUIT)));
        }
    }
    let bindings: Vec<String> = bindings
        .iter()
        .map(|(key, code)| format!("keybind {key} \"quit {code}\""))
        .collect();
    args.push(OsString::from(format!(
        "--input-commands={}",
        bindings.join(",")
    )));
}

/// Append the file after `--`, so that a name that starts with `-` is not an option.
fn push_file(args: &mut Vec<OsString>, video: &Path) {
    args.push(OsString::from("--"));
    args.push(video.as_os_str().to_owned());
}

/// The status note for an mpv exit code, or `None` when the user quit mpv or stepped on.
///
/// mpv exits with 0 when the user quits and with 4 after a signal. A process that a signal
/// killed has no code.
pub(crate) fn exit_note(code: Option<i32>, video: &Path) -> Option<String> {
    match code {
        None | Some(0 | USER_QUIT | NEXT_FILE | PREVIOUS_FILE) => None,
        Some(code) => Some(format!(
            "mpv could not play {} (exit code {code})",
            file_name(video)
        )),
    }
}

/// The file name for a status note, or the whole path if it has no file name.
pub(crate) fn file_name(video: &Path) -> String {
    video.file_name().map_or_else(
        || video.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// Starts mpv. The tests swap in a fake that records the calls.
pub(crate) trait Runner {
    /// Run `program` in the foreground with the terminal of viewr, and wait for its exit code.
    fn run(&mut self, program: &Path, args: &[OsString]) -> io::Result<Option<i32>>;
    /// Start `program` without a terminal and return at once. The receiver gets the exit code
    /// when the process ends, so that a slideshow can wait for the window without a block.
    fn spawn(
        &mut self,
        program: &Path,
        args: &[OsString],
    ) -> io::Result<mpsc::Receiver<Option<i32>>>;
}

/// The [`Runner`] that starts real processes.
pub(crate) struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&mut self, program: &Path, args: &[OsString]) -> io::Result<Option<i32>> {
        Ok(Command::new(program).args(args).status()?.code())
    }

    fn spawn(
        &mut self,
        program: &Path,
        args: &[OsString],
    ) -> io::Result<mpsc::Receiver<Option<i32>>> {
        // A process group of its own keeps the job control signals of the terminal away.
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()?;
        // Wait in a thread, so that the exited window process does not stay as a zombie.
        let (exit_tx, exit_rx) = mpsc::channel();
        thread::spawn(move || {
            let code = child.wait().ok().and_then(|status| status.code());
            #[expect(
                clippy::let_underscore_must_use,
                reason = "outside a slideshow, nobody waits for the exit of the window"
            )]
            let _ = exit_tx.send(code);
        });
        Ok(exit_rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    /// The picture of an SDR video in a terminal of 1600 pixels.
    fn sdr() -> Picture {
        Picture {
            longest_side: 1600,
            tone_map: None,
        }
    }

    const SDR_FILTER: &str = "--vf-pre=lavfi=[scale=w=min(iw\\,1600):h=min(ih\\,1600)\
        :force_original_aspect_ratio=decrease:force_divisible_by=2:flags=bilinear]";
    const STEP_BINDINGS: &str = r#"keybind n "quit 10",keybind N "quit 11""#;

    #[test]
    fn terminal_args_use_kitty_and_loop_and_end_with_the_file() {
        let args = terminal_args(Path::new("-clip.mp4"), None, Playback::Loop, &sdr());
        assert_eq!(
            strings(&args),
            [
                "--vo=kitty",
                "--vo-kitty-alt-screen=no",
                "--profile=sw-fast",
                "--hwdec=auto-copy-safe",
                "--really-quiet",
                "--terminal=yes",
                "--input-terminal=yes",
                SDR_FILTER,
                "--loop-file=inf",
                &format!("--input-commands={STEP_BINDINGS}"),
                "--",
                "-clip.mp4",
            ]
        );
    }

    #[test]
    fn terminal_args_carry_the_geometry_when_the_terminal_reports_no_pixels() {
        let geometry = fallback_geometry((100, 30, 0, 0), (8, 16));
        assert_eq!(
            geometry,
            Some(Geometry {
                cols: 100,
                rows: 30,
                width: 800,
                height: 480,
            })
        );
        let args = strings(&terminal_args(
            Path::new("/videos/a.mov"),
            geometry,
            Playback::Loop,
            &sdr(),
        ));
        let options = &args[10..14];
        assert_eq!(
            options,
            [
                "--vo-kitty-cols=100",
                "--vo-kitty-rows=30",
                "--vo-kitty-width=800",
                "--vo-kitty-height=480",
            ]
        );
        assert_eq!(&args[14..], ["--", "/videos/a.mov"]);
    }

    #[test]
    fn a_terminal_with_a_pixel_size_lets_mpv_measure() {
        assert_eq!(fallback_geometry((100, 30, 1600, 960), (8, 16)), None);
    }

    #[test]
    fn the_longest_side_comes_from_the_pixels_or_from_the_cells() {
        assert_eq!(longest_side((100, 30, 1600, 960), (8, 16)), 1600);
        assert_eq!(longest_side((100, 30, 900, 1400), (8, 16)), 1400);
        assert_eq!(longest_side((100, 30, 0, 0), (8, 16)), 800);
        assert_eq!(longest_side((40, 60, 0, 0), (8, 16)), 960);
    }

    #[test]
    fn an_hdr_video_gets_the_tone_mapping_cube_after_the_scale() {
        let picture = Picture {
            longest_side: 2000,
            tone_map: Some(PathBuf::from("/cache/tonemap-hlg-v2.cube")),
        };
        let args = strings(&terminal_args(
            Path::new("a.mov"),
            None,
            Playback::Loop,
            &picture,
        ));
        assert_eq!(
            args[7],
            "--vf-pre=lavfi=[scale=w=min(iw\\,2000):h=min(ih\\,2000):force_original_aspect_ratio=decrease\
             :force_divisible_by=2:flags=bilinear,format=rgb48le,\
             lut3d=file=/cache/tonemap-hlg-v2.cube:interp=tetrahedral,format=rgb24,\
             setparams=color_primaries=bt709:color_trc=iec61966-2-1:colorspace=gbr:range=pc]"
        );
    }

    #[test]
    fn window_args_detach_from_the_terminal() {
        assert_eq!(
            strings(&window_args(Path::new("b.webm"), Playback::Loop)),
            [
                "--no-terminal",
                "--force-window=immediate",
                "--fs",
                "--native-fs=no",
                "--hwdec=auto-safe",
                "--loop-file=inf",
                &format!("--input-commands={STEP_BINDINGS}"),
                "--",
                "b.webm",
            ]
        );
    }

    #[test]
    fn once_plays_without_the_loop_and_gives_the_quit_keys_their_own_code() {
        let terminal = strings(&terminal_args(
            Path::new("a.mp4"),
            None,
            Playback::Once,
            &sdr(),
        ));
        assert_eq!(
            terminal[8..],
            [
                "--loop-file=no",
                "--keep-open=no",
                "--idle=no",
                "--loop-playlist=no",
                &format!(
                    r#"--input-commands={STEP_BINDINGS},keybind q "quit 4",keybind Q "quit 4""#
                ),
                "--",
                "a.mp4",
            ]
        );
        let window = strings(&window_args(Path::new("b.webm"), Playback::Once));
        assert_eq!(
            window,
            [
                "--no-terminal",
                "--force-window=immediate",
                "--fs",
                "--native-fs=no",
                "--hwdec=auto-safe",
                "--loop-file=no",
                "--keep-open=no",
                "--idle=no",
                "--loop-playlist=no",
                &format!(
                    r#"--input-commands={STEP_BINDINGS},keybind q "quit 4",keybind Q "quit 4",keybind CLOSE_WIN "quit 4""#
                ),
                "--",
                "b.webm",
            ]
        );
    }

    #[test]
    fn the_step_keys_have_their_own_exit_codes() {
        assert_eq!(step(Some(NEXT_FILE)), Some(Step::Next));
        assert_eq!(step(Some(PREVIOUS_FILE)), Some(Step::Previous));
        for other in [Some(0), Some(USER_QUIT), Some(2), None] {
            assert_eq!(step(other), None);
        }
        assert!(!reached_the_end(Some(NEXT_FILE)));
        assert_eq!(exit_note(Some(NEXT_FILE), Path::new("a.mp4")), None);
        assert_eq!(exit_note(Some(PREVIOUS_FILE), Path::new("a.mp4")), None);
    }

    /// The output value of the cube for one signal value on all three channels.
    fn gray(hdr: Hdr, signal: f64) -> [f64; 3] {
        tone_map(hdr, [signal; 3])
    }

    #[test]
    fn the_tone_mapping_keeps_black_and_gray_and_rolls_off_the_highlights() {
        for hdr in [Hdr::Hlg, Hdr::Pq] {
            assert_eq!(gray(hdr, 0.0), [0.0; 3]);
            let mut last = 0.0;
            for step in 1..=20 {
                let [r, g, b] = gray(hdr, f64::from(step) / 20.0);
                assert!(
                    (r - g).abs() < 1e-3 && (g - b).abs() < 1e-3,
                    "gray stays gray"
                );
                // PQ above the 1000 nit peak stays at white.
                assert!(r >= last && r <= 1.0, "brighter in, never darker out");
                last = r;
            }
        }
        // HLG reference white (75 %, 203 nits) lands in the upper midtones, not at the clip.
        let [white, ..] = gray(Hdr::Hlg, 0.75);
        assert!(
            (0.55..0.7).contains(&white),
            "HLG reference white maps to {white}"
        );
        // A PQ value of 1000 nits is the peak, and the peak is the SDR white.
        let [peak, ..] = gray(Hdr::Pq, 0.751_827_5);
        assert!(peak > 0.99, "PQ 1000 nits maps to {peak}");
    }

    #[test]
    fn a_color_outside_bt709_keeps_its_hue_and_stays_in_range() {
        let [r, g, b] = tone_map(Hdr::Hlg, [1.0, 0.0, 0.0]);
        assert!(
            r > 0.9 && g < 0.2 && b < 0.4,
            "BT.2020 red stays red: {r} {g} {b}"
        );
        let [r, g, b] = tone_map(Hdr::Pq, [0.0, 0.7, 0.0]);
        assert!(g > r && g > b, "BT.2020 green stays green: {r} {g} {b}");
        // A red highlight far above the PQ peak stays red at full red, not white.
        let [r, g, b] = tone_map(Hdr::Pq, [0.95, 0.6, 0.6]);
        assert!(
            (r - 1.0).abs() < 1e-9,
            "the brightest channel is at white: {r}"
        );
        assert!(
            g < 0.95 && b < 0.95,
            "the other channels keep the hue: {g} {b}"
        );
    }

    #[test]
    fn the_cube_has_a_header_and_one_line_per_point() {
        let cube = tone_map_cube(Hdr::Hlg);
        let mut lines = cube.lines();
        assert_eq!(lines.next(), Some("LUT_3D_SIZE 33"));
        assert_eq!(lines.next(), Some("0.000000 0.000000 0.000000"));
        assert_eq!(cube.lines().count(), 1 + 33 * 33 * 33);
        // Red runs fastest: the second point is a dark red.
        let second: Vec<f64> = cube
            .lines()
            .nth(2)
            .unwrap()
            .split(' ')
            .map(|value| value.parse().unwrap())
            .collect();
        assert!(second[0] > 0.0 && second[1] < second[0]);
    }

    #[test]
    fn the_cube_file_is_written_once() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        let file = tone_map_file(&[&cache], Hdr::Pq).unwrap();
        assert_eq!(file, cache.join("tonemap-pq-v2.cube"));
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            tone_map_cube(Hdr::Pq)
        );
        std::fs::write(&file, "kept").unwrap();
        assert_eq!(tone_map_file(&[&cache], Hdr::Pq), Some(file.clone()));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "kept");
        assert_eq!(
            std::fs::read_dir(&cache).unwrap().count(),
            1,
            "no partial file"
        );
    }

    #[test]
    fn a_path_with_syntax_or_without_write_access_falls_back_to_the_next_directory() {
        let dir = tempfile::tempdir().unwrap();
        let fallback = dir.path().join("fallback");
        let odd = dir.path().join("a:b,c");
        let unwritable = Path::new("/dev/null/cache");
        for first in [odd.as_path(), unwritable] {
            assert_eq!(
                tone_map_file(&[first, &fallback], Hdr::Hlg),
                Some(fallback.join("tonemap-hlg-v2.cube")),
                "{}",
                first.display()
            );
        }
        assert!(!odd.exists());
        assert_eq!(tone_map_file(&[unwritable], Hdr::Hlg), None);
    }

    #[test]
    fn a_path_with_letters_outside_ascii_keeps_the_tone_mapping() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("Jürgen Ö/Caches");
        let file = tone_map_file(&[&cache], Hdr::Hlg).unwrap();
        assert!(file.is_file());
    }

    #[test]
    fn a_file_that_is_no_video_has_no_hdr_curve() {
        ffmpeg::init().unwrap();
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), "not a video").unwrap();
        assert_eq!(hdr_of(file.path()), None);
        assert_eq!(hdr_of(Path::new("/nonexistent/a.mov")), None);
    }

    /// Read the curve of real videos: `VIEWR_TEST_HDR_VIDEO` (HLG or PQ), `VIEWR_TEST_SDR_VIDEO`.
    #[test]
    #[ignore = "needs VIEWR_TEST_HDR_VIDEO and VIEWR_TEST_SDR_VIDEO, real video files"]
    fn real_videos_report_their_hdr_curve() {
        ffmpeg::init().unwrap();
        if let Some(path) = crate::decode::real_test_file("VIEWR_TEST_HDR_VIDEO") {
            assert!(hdr_of(&path).is_some());
        }
        if let Some(path) = crate::decode::real_test_file("VIEWR_TEST_SDR_VIDEO") {
            assert_eq!(hdr_of(&path), None);
        }
    }

    #[test]
    fn only_exit_code_zero_is_the_end_of_the_file() {
        assert!(reached_the_end(Some(0)));
        assert!(!reached_the_end(Some(USER_QUIT)));
        assert!(!reached_the_end(Some(2)));
        assert!(!reached_the_end(None));
    }

    #[test]
    fn a_missing_mpv_falls_back_to_the_builtin_player() {
        let mpv = PathBuf::from("/opt/homebrew/bin/mpv");
        assert_eq!(resolve(VideoPlayer::Terminal, None), Player::Builtin);
        assert_eq!(resolve(VideoPlayer::Window, None), Player::Builtin);
        assert_eq!(
            resolve(VideoPlayer::Builtin, Some(mpv.clone())),
            Player::Builtin
        );
        assert_eq!(
            resolve(VideoPlayer::Terminal, Some(mpv.clone())),
            Player::Terminal(mpv.clone())
        );
        assert_eq!(
            resolve(VideoPlayer::Window, Some(mpv.clone())),
            Player::Window(mpv)
        );
    }

    #[test]
    fn exit_codes_of_a_quit_give_no_note() {
        let video = Path::new("/videos/c.mkv");
        assert_eq!(exit_note(Some(0), video), None);
        assert_eq!(exit_note(Some(4), video), None);
        assert_eq!(exit_note(None, video), None);
        assert_eq!(
            exit_note(Some(2), video).as_deref(),
            Some("mpv could not play c.mkv (exit code 2)")
        );
    }

    #[test]
    fn a_path_without_a_file_name_names_the_whole_path() {
        assert_eq!(file_name(Path::new("/")), "/");
        assert_eq!(file_name(Path::new("/videos/d.mp4")), "d.mp4");
    }

    #[test]
    fn system_runner_returns_the_exit_code_of_the_program() {
        let shell = Path::new("/bin/sh");
        let args = [OsString::from("-c"), OsString::from("exit 3")];
        assert_eq!(SystemRunner.run(shell, &args).unwrap(), Some(3));
        assert!(
            SystemRunner
                .run(Path::new("/nonexistent/mpv"), &[])
                .is_err()
        );
    }

    #[test]
    fn system_runner_spawns_a_detached_program_and_reports_its_exit() {
        let shell = Path::new("/bin/sh");
        let args = [OsString::from("-c"), OsString::from("exit 5")];
        let exit = SystemRunner.spawn(shell, &args).unwrap();
        assert_eq!(
            exit.recv_timeout(std::time::Duration::from_secs(10)),
            Ok(Some(5))
        );
        assert!(
            SystemRunner
                .spawn(Path::new("/nonexistent/mpv"), &[])
                .is_err()
        );
    }
}
