//! The Kitty graphics writes: the transmit through each medium, the frames, the deletes and the
//! placements.
use super::{DisplayOptions, Pixels, Transfer};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder};
use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

mod probe;

#[cfg(test)]
pub(crate) use probe::detect_transfer;
pub(crate) use probe::{
    LOOPS_OVERRIDE, TRANSFER_OVERRIDE, chosen_loops, describe, describe_loops, detect_backend,
    is_ssh,
};

const CHUNK_SIZE: usize = 4096;
/// The protocol reads a temp file only if its name holds this marker.
const TEMP_FILE_MARKER: &str = "tty-graphics-protocol";

/// Write `pixels` with the transmit-and-display action (`a=T`) through `transfer`. An opaque
/// RGBA image goes out as RGB (`f=24`).
pub(crate) fn transmit_to<W: Write + ?Sized>(
    out: &mut W,
    pixels: Pixels<'_>,
    opts: &DisplayOptions,
    transfer: Transfer,
) -> io::Result<()> {
    transmit_in(out, pixels, opts, transfer, &std::env::temp_dir())
}

/// [`transmit_to`] with the temp file directory. A test makes the write fail.
fn transmit_in<W: Write + ?Sized>(
    out: &mut W,
    pixels: Pixels<'_>,
    opts: &DisplayOptions,
    transfer: Transfer,
    temp_dir: &Path,
) -> io::Result<()> {
    with_opaque_as_rgb(pixels, transfer, |pixels| {
        let keys = Keys {
            first: &|format| header_keys(pixels, opts, format),
            chunk: "",
        };
        write_pixels(out, pixels, &keys, transfer, temp_dir)
    })
}

/// Write `pixels` as a frame of the animation `id` (`a=f`) that shows for `gap_ms`.
///
/// The temp file medium sends the frame in its own format, without the scan and the RGB copy.
/// The direct and the PNG medium keep the RGB conversion. In a frame, `c=` and `r=` name frames,
/// so the frame has no cell box and no base frame.
pub(crate) fn transmit_frame_to<W: Write + ?Sized>(
    out: &mut W,
    pixels: Pixels<'_>,
    id: u32,
    gap_ms: u32,
    transfer: Transfer,
    transient: bool,
) -> io::Result<()> {
    let pixels = Pixels {
        keep_alpha: true,
        ..pixels
    };
    with_opaque_as_rgb(pixels, transfer, |pixels| {
        let (w, h) = pixels.dimensions();
        let gap = gap_ms.max(1);
        let hint = if transient { ",N=1" } else { "" };
        let keys = Keys {
            first: &|format| format!("a=f,q=2,f={format},s={w},v={h},i={id},z={gap}{hint}"),
            chunk: "a=f,",
        };
        write_pixels(out, pixels, &keys, transfer, &std::env::temp_dir())
    })
}

/// Run the animation `id` forever (`s=3`, `v=1`). The root frame gets its gap `root_gap_ms`
/// here: its transmit has no gap key.
pub(crate) fn animate_to<W: Write + ?Sized>(
    out: &mut W,
    id: u32,
    root_gap_ms: u32,
) -> io::Result<()> {
    apc(
        out,
        format_args!("a=a,q=2,i={id},r=1,z={}", root_gap_ms.max(1)),
        b"",
    )?;
    apc(out, format_args!("a=a,q=2,i={id},s=3,v=1"), b"")
}

/// Write one command: `ESC _ G`, the keys, `;` and the payload, then `ESC \`. Without a
/// payload there is no `;`.
fn apc<W: Write + ?Sized>(out: &mut W, keys: fmt::Arguments<'_>, payload: &[u8]) -> io::Result<()> {
    out.write_fmt(format_args!("\x1b_G{keys}"))?;
    if !payload.is_empty() {
        out.write_all(b";")?;
        out.write_all(payload)?;
    }
    out.write_all(b"\x1b\\")
}

/// Call `send` with the RGB copy of opaque `pixels`, else with `pixels`.
fn with_opaque_as_rgb<T>(
    pixels: Pixels<'_>,
    transfer: Transfer,
    send: impl FnOnce(Pixels<'_>) -> T,
) -> T {
    if pixels.keep_alpha && transfer == Transfer::TempFile {
        return send(pixels);
    }
    match pixels.opaque_rgb() {
        Some(rgb) => send(Pixels {
            alpha: false,
            data: &rgb,
            ..pixels
        }),
        None => send(pixels),
    }
}

struct Keys<'a> {
    /// The keys of the first escape code, for the format key of the medium.
    first: &'a dyn Fn(u32) -> String,
    /// The keys before `m=` in each further chunk. A frame repeats `a=f` there.
    chunk: &'static str,
}

fn write_pixels<W: Write + ?Sized>(
    out: &mut W,
    pixels: Pixels<'_>,
    keys: &Keys<'_>,
    transfer: Transfer,
    temp_dir: &Path,
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
        // A full or read-only temp dir must not stop viewr. The pixels go through the pty.
        Transfer::TempFile => match write_temp_file(pixels.data, temp_dir) {
            Ok(path) => write_temp_file_reference(out, keys, raw_format(pixels), &path),
            Err(_) => write_pixels(out, pixels, keys, Transfer::Direct, temp_dir),
        },
    }
}

fn raw_format(pixels: Pixels<'_>) -> u32 {
    if pixels.alpha { 32 } else { 24 }
}

/// The keys of every transmission: the format, the size, the id and the cell box.
fn header_keys(pixels: Pixels<'_>, opts: &DisplayOptions, format: u32) -> String {
    let (w, h) = pixels.dimensions();
    let id_part = opts.id.map(|id| format!(",i={id}")).unwrap_or_default();
    let box_part = match (opts.cell_box, opts.columns) {
        (None, Some(columns)) => format!(",c={columns}"),
        (cell_box, _) => cell_box_keys(cell_box),
    };
    let action = if opts.transmit_only { 't' } else { 'T' };
    let hint = if opts.transient { ",N=1" } else { "" };
    format!("a={action},q=2,C=1,f={format},s={w},v={h}{id_part}{box_part}{hint}")
}

fn cell_box_keys(cell_box: Option<(u16, u16)>) -> String {
    cell_box
        .map(|(cols, rows)| format!(",c={cols},r={rows}"))
        .unwrap_or_default()
}

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
            apc(out, format_args!("{first},m={more}"), chunk)?;
        } else {
            apc(out, format_args!("{}m={more}", keys.chunk), chunk)?;
        }
    }

    Ok(())
}

/// The `t=t` sequence. The terminal reads the file and deletes it.
fn write_temp_file_reference<W: Write + ?Sized>(
    out: &mut W,
    keys: &Keys<'_>,
    format: u32,
    path: &Path,
) -> io::Result<()> {
    let first = (keys.first)(format);
    apc(
        out,
        format_args!("{first},t=t"),
        encode_path(path).as_bytes(),
    )
}

/// Write `pixels` to a new marked file in `temp_dir`. The protocol accepts `/tmp`, `/dev/shm`
/// and `$TMPDIR`.
fn write_temp_file(pixels: &[u8], temp_dir: &Path) -> io::Result<PathBuf> {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = temp_dir.join(format!(
        "{TEMP_FILE_MARKER}-viewr-{}-{sequence}.raw",
        std::process::id()
    ));
    if let Err(error) = std::fs::write(&path, pixels) {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "the write error matters more than a partial file"
        )]
        let _ = std::fs::remove_file(&path);
        return Err(error);
    }
    Ok(path)
}

fn encode_path(path: &Path) -> String {
    STANDARD.encode(path.as_os_str().as_encoded_bytes())
}

/// The temp file paths in `bytes`. No terminal reads them in a test, and the test deletes them.
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

/// Delete every image with its pixels and placements (`d=A`).
pub(crate) fn delete_all_to<W: Write + ?Sized>(out: &mut W) -> io::Result<()> {
    apc(out, format_args!("a=d,d=A,q=2"), b"")
}

/// Delete every placement and keep the pixels (`d=a`). A later `a=p` shows an image again.
pub(crate) fn clear_placements_to<W: Write + ?Sized>(out: &mut W) -> io::Result<()> {
    apc(out, format_args!("a=d,d=a,q=2"), b"")
}

/// Delete the image `id` with its placements (`d=I`).
pub(crate) fn delete_image_to<W: Write + ?Sized>(out: &mut W, id: u32) -> io::Result<()> {
    apc(out, format_args!("a=d,d=I,i={id},q=2"), b"")
}

/// Delete the placements of the image `id` and keep its pixels (`d=i`).
pub(crate) fn delete_placements_to<W: Write + ?Sized>(out: &mut W, id: u32) -> io::Result<()> {
    apc(out, format_args!("a=d,d=i,i={id},q=2"), b"")
}

/// Place a stored image by its id at the cursor, scaled to `cell_box`. `C=1` keeps the text
/// cursor fixed: a large placement never scrolls the grid.
pub(crate) fn place_by_id_to<W: Write + ?Sized>(
    out: &mut W,
    id: u32,
    cell_box: Option<(u16, u16)>,
) -> io::Result<()> {
    apc(
        out,
        format_args!("a=p,q=2,C=1,i={id}{}", cell_box_keys(cell_box)),
        b"",
    )
}

#[cfg(test)]
mod tests;
