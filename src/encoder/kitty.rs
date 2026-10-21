use super::{DisplayOptions, Transfer};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ImageEncoder, RgbaImage};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const CHUNK_SIZE: usize = 4096;
/// The Kitty protocol reads a temp file only if its name holds this marker.
const TEMP_FILE_MARKER: &str = "tty-graphics-protocol";
/// The image id of the probe. The terminal repeats it in its answer.
const PROBE_ID: u32 = 31;
/// The environment variable that skips the probe: `direct`, `png` or `file`.
pub(crate) const TRANSFER_OVERRIDE: &str = "VIEWR_KITTY_TRANSFER";

/// Write `img` to `out` with the Kitty transmit-and-display action, through `transfer`.
pub(crate) fn transmit_to<W: Write + ?Sized>(
    out: &mut W,
    img: &RgbaImage,
    opts: &DisplayOptions,
    transfer: Transfer,
) -> io::Result<()> {
    match transfer {
        Transfer::Direct => write_direct(out, img, opts, 32, &STANDARD.encode(img.as_raw())),
        Transfer::Png => {
            let mut png_buf = Vec::new();
            PngEncoder::new_with_quality(&mut png_buf, CompressionType::Fast, FilterType::Sub)
                .write_image(
                    img.as_raw(),
                    img.width(),
                    img.height(),
                    image::ExtendedColorType::Rgba8,
                )
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            write_direct(out, img, opts, 100, &STANDARD.encode(&png_buf))
        }
        // A temp dir that takes no file (full, read-only) must not stop the program, so the
        // pixels go through the pty instead.
        Transfer::TempFile => match write_temp_file(img.as_raw()) {
            Ok(path) => write_temp_file_reference(out, img, opts, &path),
            Err(_) => transmit_to(out, img, opts, Transfer::Direct),
        },
    }
}

/// The keys that every transmission shares: the format, the size, the id and the cell box.
fn header_keys(img: &RgbaImage, opts: &DisplayOptions, format: u32) -> String {
    let (w, h) = img.dimensions();
    let id_part = opts.id.map(|id| format!(",i={id}")).unwrap_or_default();
    let cols_part = opts.cols.map(|c| format!(",c={c}")).unwrap_or_default();
    let rows_part = opts.rows.map(|r| format!(",r={r}")).unwrap_or_default();
    let offset_part = offset_keys(opts.offset);
    format!("a=T,q=2,C=1,f={format},s={w},v={h}{id_part}{cols_part}{rows_part}{offset_part}")
}

/// The `X=` and `Y=` keys for a pixel offset inside the first cell. A zero offset needs no key.
fn offset_keys((x, y): (u32, u32)) -> String {
    let x_part = if x > 0 {
        format!(",X={x}")
    } else {
        String::new()
    };
    let y_part = if y > 0 {
        format!(",Y={y}")
    } else {
        String::new()
    };
    format!("{x_part}{y_part}")
}

/// The base64 payload in chunks of 4096 bytes through the pty.
fn write_direct<W: Write + ?Sized>(
    out: &mut W,
    img: &RgbaImage,
    opts: &DisplayOptions,
    format: u32,
    b64: &str,
) -> io::Result<()> {
    let chunks: Vec<&[u8]> = b64.as_bytes().chunks(CHUNK_SIZE).collect();
    let total = chunks.len();
    let keys = header_keys(img, opts, format);

    for (i, chunk) in chunks.iter().enumerate() {
        let more = i32::from(i + 1 < total);
        if i == 0 {
            write!(out, "\x1b_G{keys},m={more};")?;
        } else {
            write!(out, "\x1b_Gm={more};")?;
        }
        out.write_all(chunk)?;
        out.write_all(b"\x1b\\")?;
    }

    Ok(())
}

/// The sequence that names a temp file with the raw RGBA pixels (`t=t`). The terminal reads
/// the file and deletes it.
fn write_temp_file_reference<W: Write + ?Sized>(
    out: &mut W,
    img: &RgbaImage,
    opts: &DisplayOptions,
    path: &Path,
) -> io::Result<()> {
    let keys = header_keys(img, opts, 32);
    write!(out, "\x1b_G{keys},t=t;{}\x1b\\", encode_path(path))
}

/// Write `pixels` to a new file in the temp dir with the marker in its name. The protocol
/// accepts a file in `/tmp`, `/dev/shm` or `$TMPDIR`, and `temp_dir` follows `$TMPDIR`.
fn write_temp_file(pixels: &[u8]) -> io::Result<PathBuf> {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "{TEMP_FILE_MARKER}-viewr-{}-{sequence}.rgba",
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

/// `Some(true)` when `response` is the `OK` answer to the probe, `Some(false)` for an error
/// answer, `None` for anything else.
fn probe_accepted(response: &[u8]) -> Option<bool> {
    let text = String::from_utf8_lossy(response);
    let (_, answer) = text.split_once("\x1b_G")?;
    let (keys, rest) = answer.split_once(';')?;
    let status = rest.split("\x1b\\").next()?;
    let id = format!("i={PROBE_ID}");
    keys.split(',').any(|key| key == id).then(|| status == "OK")
}

/// Pick the transfer for this terminal.
///
/// `override_value` is `VIEWR_KITTY_TRANSFER`. Without it, SSH takes the PNG path. Otherwise
/// `query` sends the probe to the terminal and returns its answer. The temp file medium wins
/// only if the answer is `OK` and the terminal deleted the probe file: a terminal with another
/// `TMPDIR` than the shell reads the file but leaves it, and each frame would leave 22 MB.
/// Every other case keeps the direct transfer.
pub(crate) fn detect_transfer(
    override_value: Option<&str>,
    ssh: bool,
    query: impl FnOnce(&[u8]) -> Option<Vec<u8>>,
) -> Transfer {
    match override_value.map(|value| value.trim().to_ascii_lowercase()) {
        Some(value) if value == "direct" => return Transfer::Direct,
        Some(value) if value == "png" => return Transfer::Png,
        Some(value) if value == "file" => return Transfer::TempFile,
        _ => {}
    }
    if ssh {
        return Transfer::Png;
    }
    let Ok(path) = write_temp_file(&[0, 0, 0, 0]) else {
        return Transfer::Direct;
    };
    let answer = query(&probe_sequence(&path));
    let accepted = answer.as_deref().and_then(probe_accepted) == Some(true);
    let deleted = !path.exists();
    #[expect(
        clippy::let_underscore_must_use,
        reason = "a terminal that keeps the probe file leaves only 4 bytes of disk space"
    )]
    let _ = std::fs::remove_file(&path);
    if accepted && deleted {
        Transfer::TempFile
    } else {
        Transfer::Direct
    }
}

/// The medium for the status bar, with the key of the protocol.
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

/// Place an already-transmitted image (by ID) at the cursor position, `offset` pixels into the
/// cell. Cheap — no PNG payload. `C=1` keeps the text cursor fixed so large placements never
/// scroll the grid.
pub(crate) fn place_by_id_to<W: Write + ?Sized>(
    out: &mut W,
    id: u32,
    offset: (u32, u32),
) -> io::Result<()> {
    write!(out, "\x1b_Ga=p,q=2,C=1,i={id}{}\x1b\\", offset_keys(offset))
}

#[cfg(test)]
mod tests {
    use super::{
        PROBE_ID, TEMP_FILE_MARKER, Transfer, clear_placements_to, delete_all_to, detect_transfer,
        place_by_id_to, probe_accepted, temp_file_paths, transmit_to,
    };
    use crate::encoder::DisplayOptions;
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use image::RgbaImage;
    use std::fs;
    use std::path::PathBuf;

    fn options(id: Option<u32>) -> DisplayOptions {
        DisplayOptions {
            id,
            cols: None,
            rows: None,
            offset: (0, 0),
        }
    }

    fn transmit(image: &RgbaImage, opts: &DisplayOptions, transfer: Transfer) -> String {
        let mut output = Vec::new();
        transmit_to(&mut output, image, opts, transfer).unwrap();
        String::from_utf8(output).unwrap()
    }

    #[test]
    fn management_sequences_match_the_kitty_protocol() {
        let mut output = Vec::new();
        delete_all_to(&mut output).unwrap();
        clear_placements_to(&mut output).unwrap();
        place_by_id_to(&mut output, 42, (0, 0)).unwrap();
        assert_eq!(
            output,
            b"\x1b_Ga=d,d=A,q=2\x1b\\\x1b_Ga=d,d=a,q=2\x1b\\\x1b_Ga=p,q=2,C=1,i=42\x1b\\"
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
    fn transmission_includes_cell_size_options() {
        let encoded = transmit(
            &RgbaImage::new(2, 3),
            &DisplayOptions {
                id: None,
                cols: Some(4),
                rows: Some(2),
                offset: (0, 0),
            },
            Transfer::Direct,
        );
        assert!(encoded.contains(",c=4,r=2,"));
        assert!(!encoded.contains(",X="));
        assert!(!encoded.contains(",Y="));
    }

    #[test]
    fn a_pixel_offset_goes_out_as_x_and_y_keys() {
        let encoded = transmit(
            &RgbaImage::new(2, 3),
            &DisplayOptions {
                id: Some(3),
                cols: None,
                rows: None,
                offset: (4, 5),
            },
            Transfer::Direct,
        );
        assert!(encoded.starts_with("\x1b_Ga=T,q=2,C=1,f=32,s=2,v=3,i=3,X=4,Y=5,m=0;"));

        let mut output = Vec::new();
        place_by_id_to(&mut output, 42, (0, 7)).unwrap();
        assert_eq!(output, b"\x1b_Ga=p,q=2,C=1,i=42,Y=7\x1b\\");
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
        let image = RgbaImage::from_fn(300, 200, |x, y| image::Rgba([x as u8, y as u8, 7, 255]));
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
        assert_eq!(probe_accepted(ok.as_bytes()), Some(true));
        let error = format!("\x1b_Gi={PROBE_ID};EBADF:could not open file\x1b\\");
        assert_eq!(probe_accepted(error.as_bytes()), Some(false));
        let other_id = "\x1b_Gi=7;OK\x1b\\";
        assert_eq!(probe_accepted(other_id.as_bytes()), None);
        assert_eq!(probe_accepted(b"\x1b[0n"), None);
        assert_eq!(probe_accepted(b""), None);
        let late_keys = format!("\x1b[I\x1b_Gi={PROBE_ID},I=0;OK\x1b\\");
        assert_eq!(probe_accepted(late_keys.as_bytes()), Some(true));
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
}
