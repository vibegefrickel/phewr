use super::{DisplayOptions, KittyBackend, Loops, Pixels, Transfer};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const CHUNK_SIZE: usize = 4096;
/// The Kitty protocol reads a temp file only if its name holds this marker.
const TEMP_FILE_MARKER: &str = "tty-graphics-protocol";
/// The image id of the probe. The terminal repeats it in its answer.
const PROBE_ID: u32 = 31;
/// The image id of the animation probe. The probe deletes it in the same write, before the
/// grid sends its first picture.
const ANIMATION_PROBE_ID: u32 = 32;
/// One transparent RGBA pixel in base64, the pixels of the animation probe.
const ONE_PIXEL: &str = "AAAAAA==";
/// The environment variable that skips the probe: `direct`, `png` or `file`.
pub(crate) const TRANSFER_OVERRIDE: &str = "VIEWR_KITTY_TRANSFER";
/// The environment variable that fixes the loop path of the grid: `animation` or `ticks`. It
/// wins over `[gallery] loops`.
pub(crate) const LOOPS_OVERRIDE: &str = "VIEWR_KITTY_LOOPS";

/// Write `pixels` to `out` with the Kitty transmit-and-display action, through `transfer`.
///
/// An RGBA image without a transparent pixel goes out as RGB (`f=24`): a quarter fewer bytes
/// in the temp file or the pty, and a smaller PNG.
pub(crate) fn transmit_to<W: Write + ?Sized>(
    out: &mut W,
    pixels: Pixels<'_>,
    opts: &DisplayOptions,
    transfer: Transfer,
) -> io::Result<()> {
    with_opaque_as_rgb(pixels, |pixels| {
        let keys = Keys {
            first: &|format| header_keys(pixels, opts, format),
            chunk: "",
        };
        write_pixels(out, pixels, &keys, transfer)
    })
}

/// Write `pixels` to `out` as a new frame of the animation of the image `id` (`a=f`), through
/// `transfer`. The frame shows for `gap_ms` milliseconds when the animation runs.
///
/// The frame covers the whole image, so it needs no base frame (`c=`), and the keys of the
/// cell box and of the cursor do not apply: in a frame, `c=` and `r=` name frames.
pub(crate) fn transmit_frame_to<W: Write + ?Sized>(
    out: &mut W,
    pixels: Pixels<'_>,
    id: u32,
    gap_ms: u32,
    transfer: Transfer,
) -> io::Result<()> {
    with_opaque_as_rgb(pixels, |pixels| {
        let (w, h) = pixels.dimensions();
        let gap = gap_ms.max(1);
        let keys = Keys {
            first: &|format| format!("a=f,q=2,f={format},s={w},v={h},i={id},z={gap}"),
            chunk: "a=f,",
        };
        write_pixels(out, pixels, &keys, transfer)
    })
}

/// Run the animation of the image `id` in a loop forever (`s=3`, `v=1`). The root frame gets
/// its gap of `root_gap_ms` here, because its transmit has no gap key.
pub(crate) fn animate_to<W: Write + ?Sized>(
    out: &mut W,
    id: u32,
    root_gap_ms: u32,
) -> io::Result<()> {
    write!(
        out,
        "\x1b_Ga=a,q=2,i={id},r=1,z={}\x1b\\\x1b_Ga=a,q=2,i={id},s=3,v=1\x1b\\",
        root_gap_ms.max(1)
    )
}

/// Call `send` with the RGB copy of `pixels` when every pixel is opaque, else with `pixels`.
fn with_opaque_as_rgb<T>(pixels: Pixels<'_>, send: impl FnOnce(Pixels<'_>) -> T) -> T {
    match pixels.opaque_rgb() {
        Some(rgb) => send(Pixels {
            alpha: false,
            data: &rgb,
            ..pixels
        }),
        None => send(pixels),
    }
}

/// The control keys of one transmission.
struct Keys<'a> {
    /// The keys of the first escape code, for the format key that the medium picks.
    first: &'a dyn Fn(u32) -> String,
    /// The keys before `m=` in each further chunk. A frame repeats `a=f` there.
    chunk: &'static str,
}

/// Write `pixels` as they are, through `transfer`.
fn write_pixels<W: Write + ?Sized>(
    out: &mut W,
    pixels: Pixels<'_>,
    keys: &Keys<'_>,
    transfer: Transfer,
) -> io::Result<()> {
    match transfer {
        Transfer::Direct => {
            write_direct(out, keys, raw_format(pixels), &STANDARD.encode(pixels.data))
        }
        Transfer::Png => {
            let color = if pixels.alpha {
                ExtendedColorType::Rgba8
            } else {
                ExtendedColorType::Rgb8
            };
            let mut png_buf = Vec::new();
            PngEncoder::new_with_quality(&mut png_buf, CompressionType::Fast, FilterType::Sub)
                .write_image(pixels.data, pixels.width, pixels.height, color)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            write_direct(out, keys, 100, &STANDARD.encode(&png_buf))
        }
        // A temp dir that takes no file (full, read-only) must not stop the program, so the
        // pixels go through the pty instead.
        Transfer::TempFile => match write_temp_file(pixels.data) {
            Ok(path) => write_temp_file_reference(out, keys, raw_format(pixels), &path),
            Err(_) => write_pixels(out, pixels, keys, Transfer::Direct),
        },
    }
}

/// The format key of raw pixels: `f=24` for RGB, `f=32` for RGBA.
fn raw_format(pixels: Pixels<'_>) -> u32 {
    if pixels.alpha { 32 } else { 24 }
}

/// The keys that every transmission shares: the format, the size, the id and the cell box.
fn header_keys(pixels: Pixels<'_>, opts: &DisplayOptions, format: u32) -> String {
    let (w, h) = pixels.dimensions();
    let id_part = opts.id.map(|id| format!(",i={id}")).unwrap_or_default();
    let box_part = cell_box_keys(opts.cell_box);
    let action = if opts.transmit_only { 't' } else { 'T' };
    format!("a={action},q=2,C=1,f={format},s={w},v={h}{id_part}{box_part}")
}

/// The `c=` and `r=` keys of the cell box that the terminal scales the picture to.
fn cell_box_keys(cell_box: Option<(u16, u16)>) -> String {
    cell_box
        .map(|(cols, rows)| format!(",c={cols},r={rows}"))
        .unwrap_or_default()
}

/// The base64 payload in chunks of 4096 bytes through the pty.
fn write_direct<W: Write + ?Sized>(
    out: &mut W,
    keys: &Keys<'_>,
    format: u32,
    b64: &str,
) -> io::Result<()> {
    let chunks: Vec<&[u8]> = b64.as_bytes().chunks(CHUNK_SIZE).collect();
    let total = chunks.len();
    let first = (keys.first)(format);

    for (i, chunk) in chunks.iter().enumerate() {
        let more = i32::from(i + 1 < total);
        if i == 0 {
            write!(out, "\x1b_G{first},m={more};")?;
        } else {
            write!(out, "\x1b_G{}m={more};", keys.chunk)?;
        }
        out.write_all(chunk)?;
        out.write_all(b"\x1b\\")?;
    }

    Ok(())
}

/// The sequence that names a temp file with the raw pixels (`t=t`). The terminal reads the
/// file and deletes it.
fn write_temp_file_reference<W: Write + ?Sized>(
    out: &mut W,
    keys: &Keys<'_>,
    format: u32,
    path: &Path,
) -> io::Result<()> {
    let first = (keys.first)(format);
    write!(out, "\x1b_G{first},t=t;{}\x1b\\", encode_path(path))
}

/// Write `pixels` to a new file in the temp dir with the marker in its name. The protocol
/// accepts a file in `/tmp`, `/dev/shm` or `$TMPDIR`, and `temp_dir` follows `$TMPDIR`.
fn write_temp_file(pixels: &[u8]) -> io::Result<PathBuf> {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "{TEMP_FILE_MARKER}-viewr-{}-{sequence}.raw",
        std::process::id()
    ));
    if let Err(error) = std::fs::write(&path, pixels) {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "the write error is the one to report; a partial file is only disk space"
        )]
        let _ = std::fs::remove_file(&path);
        return Err(error);
    }
    Ok(path)
}

fn encode_path(path: &Path) -> String {
    STANDARD.encode(path.as_os_str().as_encoded_bytes())
}

/// The paths of the temp files that the sequences in `bytes` name. A test deletes them, because
/// no terminal reads them.
#[cfg(test)]
pub(crate) fn temp_file_paths(bytes: &[u8]) -> Vec<PathBuf> {
    let text = String::from_utf8_lossy(bytes);
    text.split("\x1b_G")
        .filter_map(|sequence| {
            let (keys, rest) = sequence.split_once(';')?;
            if !keys.split(',').any(|key| key == "t=t") {
                return None;
            }
            let payload = rest.split("\x1b\\").next()?;
            let decoded = STANDARD.decode(payload).ok()?;
            Some(PathBuf::from(String::from_utf8(decoded).ok()?))
        })
        .collect()
}

/// True when the terminal sits on another machine, so a file of this machine is out of reach.
pub(crate) fn is_ssh() -> bool {
    std::env::var_os("SSH_CLIENT").is_some() || std::env::var_os("SSH_CONNECTION").is_some()
}

/// The Kitty query (`a=q`) that asks whether the terminal reads a temp file.
fn probe_sequence(path: &Path) -> Vec<u8> {
    format!(
        "\x1b_Ga=q,i={PROBE_ID},t=t,f=32,s=1,v=1;{}\x1b\\",
        encode_path(path)
    )
    .into_bytes()
}

/// The sequences that ask whether the terminal plays Kitty animations: a silent 1x1 image
/// (`q=2`), one frame for it that the terminal answers, and the delete of the image.
///
/// A query (`a=q`) stores no image, so the frame needs a real one. The frame has no `q` key,
/// because `q=1` suppresses the `OK` answer.
fn animation_probe_sequence() -> Vec<u8> {
    format!(
        "\x1b_Ga=t,q=2,i={ANIMATION_PROBE_ID},f=32,s=1,v=1;{ONE_PIXEL}\x1b\\\
         \x1b_Ga=f,i={ANIMATION_PROBE_ID},f=32,s=1,v=1;{ONE_PIXEL}\x1b\\\
         \x1b_Ga=d,d=I,i={ANIMATION_PROBE_ID},q=2\x1b\\"
    )
    .into_bytes()
}

/// `Some(true)` when `response` holds the `OK` answer for the image `id`, `Some(false)` for
/// an error answer, `None` without an answer for `id`.
fn answer_for(response: &[u8], id: u32) -> Option<bool> {
    let text = String::from_utf8_lossy(response);
    let id = format!("i={id}");
    text.split("\x1b_G").skip(1).find_map(|answer| {
        let (keys, rest) = answer.split_once(';')?;
        let status = rest.split("\x1b\\").next()?;
        keys.split(',').any(|key| key == id).then(|| status == "OK")
    })
}

/// The loop path that `VIEWR_KITTY_LOOPS` or `[gallery] loops` fixes, the variable first.
/// `None` lets the probe decide.
pub(crate) fn chosen_loops(override_value: Option<&str>, config: Option<Loops>) -> Option<Loops> {
    override_value.and_then(Loops::parse).or(config)
}

/// The medium that the override or SSH fixes without a probe.
fn fixed_transfer(override_value: Option<&str>, ssh: bool) -> Option<Transfer> {
    match override_value.map(|value| value.trim().to_ascii_lowercase()) {
        Some(value) if value == "direct" => return Some(Transfer::Direct),
        Some(value) if value == "png" => return Some(Transfer::Png),
        Some(value) if value == "file" => return Some(Transfer::TempFile),
        _ => {}
    }
    ssh.then_some(Transfer::Png)
}

/// Pick the transfer for this terminal, without the animation probe.
#[cfg(test)]
pub(crate) fn detect_transfer(
    override_value: Option<&str>,
    ssh: bool,
    query: impl FnOnce(&[u8]) -> Option<Vec<u8>>,
) -> Transfer {
    detect_backend(override_value, Some(Loops::Ticks), ssh, |sequence, _| {
        query(sequence)
    })
    .transfer()
}

/// Pick the transfer and the loop path for this terminal, with one round trip at most.
///
/// `override_value` is `VIEWR_KITTY_TRANSFER`. Without it, SSH takes the PNG path. Otherwise
/// the transfer probe goes out. The temp file medium wins only if the answer is `OK` and the
/// terminal deleted the probe file: a terminal with another `TMPDIR` than the shell reads the
/// file but leaves it, and each frame would leave 22 MB. Every other case keeps the direct
/// transfer.
///
/// `loops` is the result of [`chosen_loops`]. Without one, the animation probe goes out too,
/// and only an `OK` for its frame turns the animations on. `query` sends the probes to the
/// terminal, waits for the count of answers in its second argument and returns the answers.
/// Without a probe, it does not run.
pub(crate) fn detect_backend(
    override_value: Option<&str>,
    loops: Option<Loops>,
    ssh: bool,
    query: impl FnOnce(&[u8], usize) -> Option<Vec<u8>>,
) -> KittyBackend {
    let fixed = fixed_transfer(override_value, ssh);
    let probe_file = if fixed.is_none() {
        write_temp_file(&[0, 0, 0, 0]).ok()
    } else {
        None
    };
    let mut sequence = Vec::new();
    let mut answers = 0;
    if let Some(path) = &probe_file {
        sequence.extend(probe_sequence(path));
        answers += 1;
    }
    if loops.is_none() {
        sequence.extend(animation_probe_sequence());
        answers += 1;
    }
    let answer = if answers > 0 {
        query(&sequence, answers)
    } else {
        None
    };
    let answer = answer.as_deref().unwrap_or_default();
    let transfer = fixed.unwrap_or_else(|| {
        probe_file.map_or(Transfer::Direct, |path| transfer_after_probe(answer, &path))
    });
    let loops = loops.unwrap_or(if answer_for(answer, ANIMATION_PROBE_ID) == Some(true) {
        Loops::Animation
    } else {
        Loops::Ticks
    });
    KittyBackend::new(transfer).with_loops(loops)
}

/// The temp file medium when the terminal answered `OK` and deleted the probe file at `path`,
/// else the direct transfer. Removes the file in any case.
fn transfer_after_probe(answer: &[u8], path: &Path) -> Transfer {
    let accepted = answer_for(answer, PROBE_ID) == Some(true);
    let deleted = !path.exists();
    #[expect(
        clippy::let_underscore_must_use,
        reason = "a terminal that keeps the probe file leaves only 4 bytes of disk space"
    )]
    let _ = std::fs::remove_file(path);
    if accepted && deleted {
        Transfer::TempFile
    } else {
        Transfer::Direct
    }
}

/// The medium for the help popup, with the key of the protocol.
/// The loop path for the help popup.
pub(crate) fn describe_loops(loops: Loops) -> &'static str {
    match loops {
        Loops::Animation => "Kitty animation (a=f)",
        Loops::Ticks => "ticks (a=p)",
    }
}

pub(crate) fn describe(transfer: Transfer) -> &'static str {
    match transfer {
        Transfer::Direct => "direct (t=d)",
        Transfer::Png => "PNG over SSH (t=d, f=100)",
        Transfer::TempFile => "temp file (t=t)",
    }
}

pub(crate) fn delete_all_to<W: Write + ?Sized>(out: &mut W) -> io::Result<()> {
    out.write_all(b"\x1b_Ga=d,d=A,q=2\x1b\\")
}

/// Delete visible placements only; keep stored image data (uppercase `A` would nuke storage).
pub(crate) fn clear_placements_to<W: Write + ?Sized>(out: &mut W) -> io::Result<()> {
    out.write_all(b"\x1b_Ga=d,d=a,q=2\x1b\\")
}

/// Delete the image `id` with its placements (`d=I`), so a replaced thumbnail frees its memory.
pub(crate) fn delete_image_to<W: Write + ?Sized>(out: &mut W, id: u32) -> io::Result<()> {
    write!(out, "\x1b_Ga=d,d=I,i={id},q=2\x1b\\")
}

/// Delete the placements of the image `id` and keep its pixels (`d=i`), so that a later `a=p`
/// shows it again without a transmit. A grid loop swaps its frames this way.
pub(crate) fn delete_placements_to<W: Write + ?Sized>(out: &mut W, id: u32) -> io::Result<()> {
    write!(out, "\x1b_Ga=d,d=i,i={id},q=2\x1b\\")
}

/// Place an already-transmitted image (by ID) at the cursor position, scaled to `cell_box` if
/// there is one. Cheap — no PNG payload. `C=1` keeps the text cursor fixed so large placements
/// never scroll the grid.
pub(crate) fn place_by_id_to<W: Write + ?Sized>(
    out: &mut W,
    id: u32,
    cell_box: Option<(u16, u16)>,
) -> io::Result<()> {
    write!(
        out,
        "\x1b_Ga=p,q=2,C=1,i={id}{}\x1b\\",
        cell_box_keys(cell_box)
    )
}

#[cfg(test)]
mod tests {
    use super::{
        ANIMATION_PROBE_ID, PROBE_ID, TEMP_FILE_MARKER, Transfer, animate_to, answer_for,
        chosen_loops, clear_placements_to, delete_all_to, delete_placements_to, detect_backend,
        detect_transfer, place_by_id_to, temp_file_paths, transmit_frame_to, transmit_to,
    };
    use crate::encoder::{DisplayOptions, KittyBackend, Loops, Pixels};
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use image::RgbaImage;
    use std::fs;
    use std::path::PathBuf;

    fn options(id: Option<u32>) -> DisplayOptions {
        DisplayOptions {
            id,
            ..DisplayOptions::default()
        }
    }

    fn transmit(image: &RgbaImage, opts: &DisplayOptions, transfer: Transfer) -> String {
        transmit_pixels(image.into(), opts, transfer)
    }

    fn transmit_pixels(pixels: Pixels<'_>, opts: &DisplayOptions, transfer: Transfer) -> String {
        let mut output = Vec::new();
        transmit_to(&mut output, pixels, opts, transfer).unwrap();
        String::from_utf8(output).unwrap()
    }

    /// The image inside the base64 PNG payload of a one-chunk sequence.
    fn png_payload(encoded: &str) -> image::DynamicImage {
        let (_, rest) = encoded.split_once(';').unwrap();
        let png = STANDARD.decode(rest.trim_end_matches("\x1b\\")).unwrap();
        assert!(png.starts_with(b"\x89PNG"));
        image::load_from_memory(&png).unwrap()
    }

    #[test]
    fn management_sequences_match_the_kitty_protocol() {
        let mut output = Vec::new();
        delete_all_to(&mut output).unwrap();
        clear_placements_to(&mut output).unwrap();
        place_by_id_to(&mut output, 42, None).unwrap();
        delete_placements_to(&mut output, 42).unwrap();
        assert_eq!(
            output,
            b"\x1b_Ga=d,d=A,q=2\x1b\\\x1b_Ga=d,d=a,q=2\x1b\\\x1b_Ga=p,q=2,C=1,i=42\x1b\\\x1b_Ga=d,d=i,i=42,q=2\x1b\\"
        );
    }

    #[test]
    fn transmission_includes_dimensions_and_id() {
        let encoded = transmit(&RgbaImage::new(2, 3), &options(Some(7)), Transfer::Direct);
        assert!(encoded.starts_with("\u{1b}_Ga=T,q=2,"));
        assert!(encoded.contains(",C=1,"));
        assert!(encoded.contains(",f=32,s=2,v=3,i=7,"));
        assert!(encoded.ends_with("\u{1b}\\"));
    }

    #[test]
    fn a_transmit_without_a_placement_takes_the_action_t() {
        let encoded = transmit(
            &RgbaImage::new(2, 3),
            &DisplayOptions {
                id: Some(9),
                transmit_only: true,
                ..DisplayOptions::default()
            },
            Transfer::Direct,
        );
        assert!(encoded.starts_with("\u{1b}_Ga=t,q=2,"), "{encoded}");
        assert!(encoded.contains(",i=9"));
    }

    #[test]
    fn transmission_includes_cell_size_options() {
        let encoded = transmit(
            &RgbaImage::new(2, 3),
            &DisplayOptions {
                id: None,
                cell_box: Some((4, 2)),
                transmit_only: false,
            },
            Transfer::Direct,
        );
        assert!(encoded.contains(",c=4,r=2,"));
        assert!(!encoded.contains(",X="));
        assert!(!encoded.contains(",Y="));

        let mut output = Vec::new();
        place_by_id_to(&mut output, 42, Some((4, 2))).unwrap();
        assert_eq!(output, b"\x1b_Ga=p,q=2,C=1,i=42,c=4,r=2\x1b\\");
    }

    #[test]
    fn the_direct_transfer_carries_the_raw_pixels_as_base64() {
        let image = RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 4]));
        let encoded = transmit(&image, &options(None), Transfer::Direct);
        assert_eq!(
            encoded,
            format!(
                "\x1b_Ga=T,q=2,C=1,f=32,s=1,v=1,m=0;{}\x1b\\",
                STANDARD.encode([1, 2, 3, 4])
            )
        );
    }

    #[test]
    fn the_png_transfer_carries_a_png() {
        let image = RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 4]));
        let encoded = transmit(&image, &options(None), Transfer::Png);
        assert!(encoded.starts_with("\x1b_Ga=T,q=2,C=1,f=100,s=1,v=1,m=0;"));
        let payload = encoded
            .trim_start_matches("\x1b_Ga=T,q=2,C=1,f=100,s=1,v=1,m=0;")
            .trim_end_matches("\x1b\\");
        let png = STANDARD.decode(payload).unwrap();
        assert!(png.starts_with(b"\x89PNG"));
    }

    #[test]
    fn an_opaque_image_goes_out_as_rgb_through_every_medium() {
        let image = RgbaImage::from_fn(3, 2, |x, y| image::Rgba([x as u8, y as u8, 7, 255]));
        let rgb: Vec<u8> = image
            .pixels()
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2]])
            .collect();

        let direct = transmit(&image, &options(None), Transfer::Direct);
        assert_eq!(
            direct,
            format!(
                "\x1b_Ga=T,q=2,C=1,f=24,s=3,v=2,m=0;{}\x1b\\",
                STANDARD.encode(&rgb)
            )
        );

        let png = png_payload(&transmit(&image, &options(None), Transfer::Png));
        assert_eq!(png.color(), image::ColorType::Rgb8);
        assert_eq!(png.as_bytes(), rgb);

        let file = transmit(&image, &options(None), Transfer::TempFile);
        assert!(
            file.starts_with("\x1b_Ga=T,q=2,C=1,f=24,s=3,v=2,t=t;"),
            "{file}"
        );
        let paths = temp_file_paths(file.as_bytes());
        assert_eq!(fs::read(&paths[0]).unwrap(), rgb);
        fs::remove_file(&paths[0]).unwrap();
    }

    #[test]
    fn one_transparent_pixel_keeps_the_alpha_channel_through_every_medium() {
        let mut image = RgbaImage::from_pixel(3, 2, image::Rgba([9, 8, 7, 255]));
        image.put_pixel(2, 1, image::Rgba([9, 8, 7, 254]));

        let direct = transmit(&image, &options(None), Transfer::Direct);
        assert_eq!(
            direct,
            format!(
                "\x1b_Ga=T,q=2,C=1,f=32,s=3,v=2,m=0;{}\x1b\\",
                STANDARD.encode(image.as_raw())
            )
        );

        let png = png_payload(&transmit(&image, &options(None), Transfer::Png));
        assert_eq!(png.color(), image::ColorType::Rgba8);
        assert_eq!(png.as_bytes(), image.as_raw().as_slice());

        let file = transmit(&image, &options(None), Transfer::TempFile);
        assert!(
            file.starts_with("\x1b_Ga=T,q=2,C=1,f=32,s=3,v=2,t=t;"),
            "{file}"
        );
        let paths = temp_file_paths(file.as_bytes());
        assert_eq!(fs::read(&paths[0]).unwrap(), *image.as_raw());
        fs::remove_file(&paths[0]).unwrap();
    }

    #[test]
    fn rgb_pixels_go_out_as_they_are() {
        let image = image::RgbImage::from_fn(2, 2, |x, y| image::Rgb([x as u8, y as u8, 3]));
        let encoded = transmit_pixels((&image).into(), &options(Some(4)), Transfer::Direct);
        assert_eq!(
            encoded,
            format!(
                "\x1b_Ga=T,q=2,C=1,f=24,s=2,v=2,i=4,m=0;{}\x1b\\",
                STANDARD.encode(image.as_raw())
            )
        );
    }

    #[test]
    fn large_payload_is_split_into_chunks() {
        // Non-uniform 2000x1 RGBA data: the raw base64 path and the PNG path both produce
        // more than 4096 bytes of payload, so the encoder must chunk.
        let image = RgbaImage::from_fn(2000, 1, |x, _y| {
            image::Rgba([
                (x & 0xFF) as u8,
                ((x * 7) & 0xFF) as u8,
                ((x * 13) & 0xFF) as u8,
                255,
            ])
        });
        for transfer in [Transfer::Direct, Transfer::Png] {
            let encoded = transmit(&image, &options(None), transfer);
            let first_semicolon = encoded.find(';').expect("semicolon after first header");
            let (first_header, _rest) = encoded.split_at(first_semicolon);
            assert!(
                first_header.ends_with(",m=1"),
                "first header should request more chunks"
            );
            assert!(
                encoded.contains("\x1b_Gm=0;"),
                "last chunk should not request more"
            );
        }
    }

    #[test]
    fn the_temp_file_transfer_writes_the_pixels_to_a_marked_file() {
        let image =
            RgbaImage::from_fn(300, 200, |x, y| image::Rgba([x as u8, y as u8, 7, x as u8]));
        let encoded = transmit(&image, &options(Some(5)), Transfer::TempFile);

        assert!(
            encoded.starts_with("\x1b_Ga=T,q=2,C=1,f=32,s=300,v=200,i=5,t=t;"),
            "{encoded}"
        );
        assert!(encoded.ends_with("\x1b\\"));
        assert_eq!(
            encoded.matches("\x1b_G").count(),
            1,
            "one sequence, no chunks"
        );
        assert!(
            encoded.len() < 400,
            "the sequence names the file, not the pixels"
        );

        let paths = temp_file_paths(encoded.as_bytes());
        assert_eq!(paths.len(), 1);
        let path = &paths[0];
        assert!(path.starts_with(std::env::temp_dir()));
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .contains(TEMP_FILE_MARKER)
        );
        assert_eq!(fs::read(path).unwrap(), *image.as_raw());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn each_transfer_has_a_note_with_its_protocol_key() {
        assert_eq!(super::describe(Transfer::Direct), "direct (t=d)");
        assert!(super::describe(Transfer::Png).contains("f=100"));
        assert!(super::describe(Transfer::TempFile).contains("t=t"));
    }

    #[test]
    fn temp_file_paths_ignores_the_other_sequences() {
        let bytes = b"\x1b_Ga=d,d=A,q=2\x1b\\\x1b_Ga=T,f=32,s=1,v=1,m=0;AAAA\x1b\\";
        assert!(temp_file_paths(bytes).is_empty());
    }

    #[test]
    fn the_probe_answer_decides_between_ok_and_error() {
        let ok = format!("\x1b_Gi={PROBE_ID};OK\x1b\\");
        assert_eq!(answer_for(ok.as_bytes(), PROBE_ID), Some(true));
        let error = format!("\x1b_Gi={PROBE_ID};EBADF:could not open file\x1b\\");
        assert_eq!(answer_for(error.as_bytes(), PROBE_ID), Some(false));
        let other_id = "\x1b_Gi=7;OK\x1b\\";
        assert_eq!(answer_for(other_id.as_bytes(), PROBE_ID), None);
        assert_eq!(answer_for(b"\x1b[0n", PROBE_ID), None);
        assert_eq!(answer_for(b"", PROBE_ID), None);
        let late_keys = format!("\x1b[I\x1b_Gi={PROBE_ID},I=0;OK\x1b\\");
        assert_eq!(answer_for(late_keys.as_bytes(), PROBE_ID), Some(true));
    }

    /// The probe as the terminal sees it, and the answer that the fake terminal gives. A real
    /// terminal deletes the probe file after it read it; the fake does the same.
    fn fake_terminal(
        answer: Option<&'static str>,
        seen: &mut Vec<u8>,
    ) -> impl FnOnce(&[u8]) -> Option<Vec<u8>> {
        move |probe: &[u8]| {
            seen.extend_from_slice(probe);
            fs::remove_file(probe_file(probe)).unwrap();
            answer.map(|answer| answer.replace("{id}", &PROBE_ID.to_string()).into_bytes())
        }
    }

    /// A terminal that reads the probe file and answers `OK`, but leaves the file.
    fn terminal_without_delete(seen: &mut Vec<u8>) -> impl FnOnce(&[u8]) -> Option<Vec<u8>> {
        move |probe: &[u8]| {
            seen.extend_from_slice(probe);
            Some(format!("\x1b_Gi={PROBE_ID};OK\x1b\\").into_bytes())
        }
    }

    fn probe_file(seen: &[u8]) -> PathBuf {
        let text = String::from_utf8_lossy(seen);
        let (_, payload) = text.split_once(';').unwrap();
        let payload = payload.trim_end_matches("\x1b\\");
        PathBuf::from(String::from_utf8(STANDARD.decode(payload).unwrap()).unwrap())
    }

    #[test]
    fn an_ok_answer_turns_the_temp_file_medium_on() {
        let mut seen = Vec::new();
        let transfer = detect_transfer(
            None,
            false,
            fake_terminal(Some("\x1b_Gi={id};OK\x1b\\"), &mut seen),
        );
        assert_eq!(transfer, Transfer::TempFile);

        let text = String::from_utf8_lossy(&seen);
        assert!(
            text.starts_with(&format!("\x1b_Ga=q,i={PROBE_ID},t=t,f=32,s=1,v=1;")),
            "{text}"
        );
        let path = probe_file(&seen);
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .contains(TEMP_FILE_MARKER)
        );
        assert!(!path.exists(), "the probe file goes away after the answer");
    }

    #[test]
    fn an_ok_answer_without_the_delete_keeps_the_direct_transfer() {
        let mut seen = Vec::new();
        let transfer = detect_transfer(None, false, terminal_without_delete(&mut seen));
        assert_eq!(transfer, Transfer::Direct);
        assert!(
            !probe_file(&seen).exists(),
            "viewr removes the probe file itself"
        );
    }

    #[test]
    fn an_error_or_no_answer_keeps_the_direct_transfer() {
        let mut seen = Vec::new();
        let error = fake_terminal(Some("\x1b_Gi={id};ENOENT:no file\x1b\\"), &mut seen);
        assert_eq!(detect_transfer(None, false, error), Transfer::Direct);
        assert!(!probe_file(&seen).exists());

        let mut seen = Vec::new();
        let silent = fake_terminal(None, &mut seen);
        assert_eq!(detect_transfer(None, false, silent), Transfer::Direct);
        assert!(
            !probe_file(&seen).exists(),
            "a timeout removes the probe file"
        );
    }

    #[test]
    fn ssh_takes_the_png_path_without_a_probe() {
        let mut seen = Vec::new();
        let transfer = detect_transfer(
            None,
            true,
            fake_terminal(Some("\x1b_Gi={id};OK\x1b\\"), &mut seen),
        );
        assert_eq!(transfer, Transfer::Png);
        assert!(seen.is_empty(), "no probe over SSH");
    }

    #[test]
    fn the_override_skips_the_probe() {
        for (value, expected) in [
            ("direct", Transfer::Direct),
            (" File ", Transfer::TempFile),
            ("PNG", Transfer::Png),
        ] {
            let mut seen = Vec::new();
            let transfer = detect_transfer(
                Some(value),
                true,
                fake_terminal(Some("\x1b_Gi={id};OK\x1b\\"), &mut seen),
            );
            assert_eq!(transfer, expected, "{value}");
            assert!(seen.is_empty(), "no probe with an override");
        }

        let mut seen = Vec::new();
        let transfer = detect_transfer(
            Some("bogus"),
            false,
            fake_terminal(Some("\x1b_Gi={id};OK\x1b\\"), &mut seen),
        );
        assert_eq!(transfer, Transfer::TempFile, "an unknown value probes");
        assert!(!seen.is_empty());
    }

    #[test]
    fn a_frame_names_its_image_and_gap_in_every_chunk() {
        let image = RgbaImage::from_pixel(64, 64, image::Rgba([1, 2, 3, 4]));
        let mut out = Vec::new();
        transmit_frame_to(&mut out, (&image).into(), 9, 83, Transfer::Direct).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.starts_with("\x1b_Ga=f,q=2,f=32,s=64,v=64,i=9,z=83,m=1;"),
            "{text}"
        );
        let chunks = text.matches("\x1b_G").count();
        assert!(chunks > 1);
        assert_eq!(
            text.matches("\x1b_Ga=f,").count(),
            chunks,
            "each chunk repeats a=f"
        );
        assert!(!text.contains("C=1") && !text.contains(",c=") && !text.contains(",r="));

        let opaque = RgbaImage::from_pixel(2, 1, image::Rgba([1, 2, 3, 255]));
        let mut out = Vec::new();
        transmit_frame_to(&mut out, (&opaque).into(), 9, 0, Transfer::TempFile).unwrap();
        for path in temp_file_paths(&out) {
            assert_eq!(fs::read(&path).unwrap(), [1, 2, 3, 1, 2, 3]);
            fs::remove_file(path).unwrap();
        }
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.starts_with("\x1b_Ga=f,q=2,f=24,s=2,v=1,i=9,z=1,t=t;"),
            "an opaque frame goes out as RGB, and a gap of 0 ms becomes 1 ms: {text}"
        );
    }

    #[test]
    fn the_animation_gets_the_gap_of_its_root_frame_and_loops_forever() {
        let mut out = Vec::new();
        animate_to(&mut out, 9, 333).unwrap();
        assert_eq!(
            out,
            b"\x1b_Ga=a,q=2,i=9,r=1,z=333\x1b\\\x1b_Ga=a,q=2,i=9,s=3,v=1\x1b\\"
        );
    }

    #[test]
    fn the_env_variable_wins_over_the_config_key() {
        assert_eq!(chosen_loops(None, None), None, "the probe decides");
        assert_eq!(
            chosen_loops(Some("ticks"), Some(Loops::Animation)),
            Some(Loops::Ticks)
        );
        assert_eq!(
            chosen_loops(Some(" Animation "), None),
            Some(Loops::Animation)
        );
        assert_eq!(
            chosen_loops(Some("auto"), Some(Loops::Ticks)),
            Some(Loops::Ticks),
            "an unknown value keeps the config"
        );
    }

    /// A terminal that answers the start probes with `answers`, and records the sequence and
    /// the count of answers that viewr waits for. It deletes the probe file, as a real one does.
    fn probed(
        transfer_override: Option<&str>,
        loops: Option<Loops>,
        ssh: bool,
        answers: Option<&str>,
    ) -> (KittyBackend, String, Option<usize>) {
        let mut seen = Vec::new();
        let mut count = None;
        let backend = detect_backend(transfer_override, loops, ssh, |sequence, owed| {
            seen.extend_from_slice(sequence);
            count = Some(owed);
            for path in temp_file_paths(sequence) {
                fs::remove_file(path).unwrap();
            }
            answers.map(|text| text.as_bytes().to_vec())
        });
        (backend, String::from_utf8(seen).unwrap(), count)
    }

    #[test]
    fn an_ok_for_the_probe_frame_turns_the_animations_on() {
        let answers = format!("\x1b_Gi={PROBE_ID};OK\x1b\\\x1b_Gi={ANIMATION_PROBE_ID};OK\x1b\\");
        let (backend, sequence, count) = probed(None, None, false, Some(&answers));
        assert_eq!(backend.transfer(), Transfer::TempFile);
        assert_eq!(backend.loops(), Loops::Animation);
        assert_eq!(count, Some(2), "one round trip for both probes");

        let id = ANIMATION_PROBE_ID;
        let root = sequence
            .find(&format!("\x1b_Ga=t,q=2,i={id},f=32,s=1,v=1;AAAAAA==\x1b\\"))
            .expect("a silent root image");
        let frame = sequence
            .find(&format!("\x1b_Ga=f,i={id},f=32,s=1,v=1;AAAAAA==\x1b\\"))
            .expect("a frame that the terminal answers");
        let delete = sequence
            .find(&format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\"))
            .unwrap();
        assert!(
            sequence.starts_with("\x1b_Ga=q,i=31,") && root < frame && frame < delete,
            "{sequence}"
        );
    }

    #[test]
    fn an_error_or_no_answer_for_the_probe_frame_keeps_the_ticks() {
        let error = format!(
            "\x1b_Gi={PROBE_ID};OK\x1b\\\x1b_Gi={ANIMATION_PROBE_ID};EINVAL:unsupported action\x1b\\"
        );
        let only_transfer = format!("\x1b_Gi={PROBE_ID};OK\x1b\\");
        for answers in [Some(error.as_str()), Some(only_transfer.as_str()), None] {
            let (backend, _, _) = probed(None, None, false, answers);
            assert_eq!(backend.loops(), Loops::Ticks, "{answers:?}");
        }
        let (backend, _, _) = probed(None, None, false, Some(&only_transfer));
        assert_eq!(
            backend.transfer(),
            Transfer::TempFile,
            "the transfer answer still counts"
        );
    }

    #[test]
    fn the_animation_probe_goes_out_alone_without_the_transfer_probe() {
        let answers = format!("\x1b_Gi={ANIMATION_PROBE_ID};OK\x1b\\");
        for (transfer_override, ssh, expected) in [
            (None, true, Transfer::Png),
            (Some("direct"), false, Transfer::Direct),
        ] {
            let (backend, sequence, count) = probed(transfer_override, None, ssh, Some(&answers));
            assert_eq!(backend.transfer(), expected);
            assert_eq!(backend.loops(), Loops::Animation);
            assert_eq!(count, Some(1));
            assert!(!sequence.contains("a=q"), "{sequence}");
        }
    }

    #[test]
    fn a_fixed_loop_path_sends_no_animation_probe() {
        let answers = format!("\x1b_Gi={PROBE_ID};OK\x1b\\\x1b_Gi={ANIMATION_PROBE_ID};OK\x1b\\");
        let (backend, sequence, count) = probed(None, Some(Loops::Ticks), false, Some(&answers));
        assert_eq!(backend.loops(), Loops::Ticks);
        assert_eq!(count, Some(1));
        assert!(!sequence.contains("a=f"), "{sequence}");

        let (backend, _, count) = probed(Some("png"), Some(Loops::Animation), false, None);
        assert_eq!(
            (backend.transfer(), backend.loops()),
            (Transfer::Png, Loops::Animation)
        );
        assert_eq!(count, None, "no probe at all");
    }

    #[test]
    fn the_answer_for_an_id_is_found_among_several() {
        let answers = format!(
            "\x1b_Gi={PROBE_ID};OK\x1b\\\x1b[6;43;18t\x1b_Gi={ANIMATION_PROBE_ID};ENOENT:no image\x1b\\"
        );
        assert_eq!(answer_for(answers.as_bytes(), PROBE_ID), Some(true));
        assert_eq!(
            answer_for(answers.as_bytes(), ANIMATION_PROBE_ID),
            Some(false)
        );
        assert_eq!(answer_for(answers.as_bytes(), 7), None);
    }
}
