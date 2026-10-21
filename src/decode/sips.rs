//! Camera RAW, Photoshop, PDF, SVG and the other `ImageIO` formats through macOS `/usr/bin/sips`.
//!
//! sips runs in a subprocess and writes an uncompressed TIFF into a temp file. The `image` crate
//! reads the TIFF (8 or 16 bit). sips keeps the stored pixel layout and writes the EXIF
//! orientation as the TIFF Orientation tag, so the TIFF decode turns the image upright. A TIFF is
//! faster than a PNG: a full 24 MP photo takes 0.16 s against 0.91 s, because sips does not
//! compress it.
//!
//! `sips -Z` also enlarges a small image, so the route reads the size first. A raster image gets
//! a smaller copy only when it is larger than the box. A PDF page and an SVG are vector graphics:
//! sips draws them again at the requested size, so they fill the box and the zoom gets a large
//! copy. A PDF page has a transparent background, so the route puts it on white paper.

use crate::decode::raster::decode_upright;
use crate::decode::resize::scaled_down_size;
use image::{DynamicImage, ImageReader, Rgba};
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const SIPS: &str = "/usr/bin/sips";

/// The vector formats: sips draws them sharp at each size.
const VECTOR_EXTENSIONS: &[&str] = &["pdf", "svg"];

/// The longest side of the zoom source of a PDF page or an SVG. sips writes them at 72 dpi by
/// default, and a letter page is then only 612x792 px.
const VECTOR_ZOOM_SIDE: u32 = 4096;

/// Decode `path` with sips. `target` is the box of the grid cell or the fullscreen view: sips
/// then writes a copy that covers it. Without a target (the zoom) sips converts the full image.
pub(crate) fn decode(path: &Path, target: Option<(u32, u32)>) -> io::Result<DynamicImage> {
    let path = &argument_path(path);
    let max_side = requested_side(path, target)?.map(|side| side.to_string());
    let tiff = tempfile::Builder::new()
        .prefix("viewr-sips-")
        .suffix(".tiff")
        .tempfile()?;
    let mut args = vec![OsStr::new("-s"), OsStr::new("format"), OsStr::new("tiff")];
    if let Some(side) = &max_side {
        args.extend([OsStr::new("-Z"), OsStr::new(side)]);
    }
    args.extend([
        path.as_os_str(),
        OsStr::new("--out"),
        tiff.path().as_os_str(),
    ]);
    run(&args)?;
    let reader = ImageReader::open(tiff.path())?.with_guessed_format()?;
    let image =
        decode_upright(reader).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(if has_extension(path, &["pdf"]) {
        on_white(&image)
    } else {
        image
    })
}

/// sips reads an argument that starts with `-` as a function name. A relative path from the
/// command line can do that (`viewr -- -x.cr2`), so it gets a `./` in front.
fn argument_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        Path::new(".").join(path)
    }
}

/// The side for `sips -Z`, or `None` for the full image. A raster zoom needs no size query.
fn requested_side(path: &Path, target: Option<(u32, u32)>) -> io::Result<Option<u32>> {
    let vector = has_extension(path, VECTOR_EXTENSIONS);
    if target.is_none() && !vector {
        return Ok(None);
    }
    let stored = stored_size(path)?;
    Ok(match (target, vector) {
        (Some(target), false) => covering_side(stored, target),
        (Some(target), true) => Some(enlarged_side(stored, target)),
        (None, _) => Some(stored.0.max(stored.1).max(VECTOR_ZOOM_SIDE)),
    })
}

fn has_extension(path: &Path, list: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| list.iter().any(|e| ext.eq_ignore_ascii_case(e)))
}

/// The longest side for `sips -Z` that covers `target` after the EXIF orientation. The route
/// does not know the orientation, so the side must cover the stored and the turned size. `None`
/// when the full image is not larger than that side.
fn covering_side(stored: (u32, u32), target: (u32, u32)) -> Option<u32> {
    let longest = |(w, h): (u32, u32)| w.max(h);
    let fitted = |size: (u32, u32)| scaled_down_size(size, target).map_or(longest(size), longest);
    let side = fitted(stored).max(fitted((stored.1, stored.0)));
    (side < longest(stored)).then_some(side)
}

/// The longest side of a vector image that fills `target`, larger or smaller than its own size.
fn enlarged_side(size: (u32, u32), target: (u32, u32)) -> u32 {
    let (w, h) = (f64::from(size.0.max(1)), f64::from(size.1.max(1)));
    let scale = f64::min(f64::from(target.0) / w, f64::from(target.1) / h);
    (w.max(h) * scale).round().max(1.0) as u32
}

/// Put a transparent PDF page on white paper, so that black text shows on a dark terminal.
fn on_white(image: &DynamicImage) -> DynamicImage {
    let mut page = image.to_rgba8();
    for Rgba([r, g, b, a]) in page.pixels_mut() {
        let alpha = u16::from(*a);
        let blend = |c: u8| ((u16::from(c) * alpha + 255 * (255 - alpha) + 127) / 255) as u8;
        (*r, *g, *b, *a) = (blend(*r), blend(*g), blend(*b), 255);
    }
    DynamicImage::ImageRgba8(page)
}

/// The size for the info panel.
pub(crate) fn dimensions(path: &Path) -> Option<(u32, u32)> {
    stored_size(&argument_path(path)).ok()
}

/// The stored pixel size, before the EXIF orientation.
fn stored_size(path: &Path) -> io::Result<(u32, u32)> {
    let args = [
        OsStr::new("-g"),
        OsStr::new("pixelWidth"),
        OsStr::new("-g"),
        OsStr::new("pixelHeight"),
        path.as_os_str(),
    ];
    let output = run(&args)?;
    parse_size(&String::from_utf8_lossy(&output.stdout)).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("sips reads no image size: {}", path.display()),
        )
    })
}

/// Read `pixelWidth: 794` and `pixelHeight: 374` from the output of `sips -g`. A PDF and an SVG
/// have a size in points with decimals (`pixelWidth: 612.000`), which rounds up. sips writes
/// `<nil>` for a file that it cannot read.
fn parse_size(stdout: &str) -> Option<(u32, u32)> {
    let value = |key: &str| {
        stdout.lines().find_map(|line| {
            let (name, value) = line.trim().split_once(':')?;
            (name == key).then(|| pixels(value.trim()))?
        })
    };
    Some((value("pixelWidth")?, value("pixelHeight")?))
}

/// A size value of sips as whole pixels. `None` for `<nil>`, zero, a negative or a huge value.
fn pixels(value: &str) -> Option<u32> {
    let value = value.parse::<f64>().ok()?.ceil();
    (value >= 1.0 && value <= f64::from(u32::MAX)).then_some(value as u32)
}

/// Run sips with `args`. A failure returns its exit status and stderr.
fn run(args: &[&OsStr]) -> io::Result<Output> {
    let output = Command::new(SIPS)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| io::Error::new(e.kind(), format!("{SIPS}: {e}")))?;
    if output.status.success() {
        return Ok(output);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        format!("sips failed ({}): {}", output.status, stderr.trim()),
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        SIPS, VECTOR_ZOOM_SIDE, argument_path, covering_side, decode, dimensions, enlarged_side,
        parse_size,
    };
    use crate::decode::resize::load_and_resize;
    use crate::decode::{EmbeddedThumbnail, decode_image_with_hint};
    use image::{DynamicImage, GenericImageView, ImageBuffer, Rgb, RgbImage};
    use ratatui::layout::Rect;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    /// A minimal little-endian TIFF block with one IFD entry: Orientation = 6 (rotate 90 CW).
    const EXIF_ORIENTATION_6: [u8; 26] = [
        b'I', b'I', 0x2A, 0x00, 0x08, 0x00, 0x00, 0x00, 0x01, 0x00, 0x12, 0x01, 0x03, 0x00, 0x01,
        0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];

    /// 64x32 pixels, red on the left and blue on the right.
    fn red_left_blue_right() -> RgbImage {
        RgbImage::from_fn(64, 32, |x, _| {
            Rgb(if x < 32 { [255, 0, 0] } else { [0, 0, 255] })
        })
    }

    /// `red_left_blue_right` as a PNG, converted by sips to `format`.
    fn sips_file(dir: &Path, format: &str) -> PathBuf {
        let png = dir.join("source.png");
        red_left_blue_right().save(&png).unwrap();
        convert(&png, format)
    }

    /// Convert `source` with sips to `format`, next to the source.
    fn convert(source: &Path, format: &str) -> PathBuf {
        let out = source.with_file_name(format!("synthetic.{format}"));
        let status = Command::new(SIPS)
            .args(["-s", "format", format])
            .arg(source)
            .arg("--out")
            .arg(&out)
            .output()
            .unwrap()
            .status;
        assert!(status.success(), "sips cannot write {format}");
        out
    }

    fn assert_red_left_blue_right(image: &DynamicImage) {
        let rgba = image.to_rgba8();
        let (w, h) = rgba.dimensions();
        let near = |pixel: [u8; 4], want: [u8; 3]| {
            pixel
                .iter()
                .zip(want)
                .all(|(&got, want)| got.abs_diff(want) < 60)
        };
        assert!(near(rgba.get_pixel(w / 4, h / 2).0, [255, 0, 0]));
        assert!(near(rgba.get_pixel(w * 3 / 4, h / 2).0, [0, 0, 255]));
    }

    #[test]
    fn the_size_comes_from_the_sips_output() {
        let stdout = "/tmp/a.psd\n  pixelWidth: 794\n  pixelHeight: 374\n";
        assert_eq!(parse_size(stdout), Some((794, 374)));
        // A PDF or an SVG has a size in points with decimals.
        let vector = "  pixelWidth: 612.000\n  pixelHeight: 791.500\n";
        assert_eq!(parse_size(vector), Some((612, 792)));
        assert_eq!(
            parse_size("  pixelWidth: <nil>\n  pixelHeight: <nil>\n"),
            None
        );
        assert_eq!(parse_size("  pixelWidth: 0\n  pixelHeight: -3\n"), None);
        assert_eq!(parse_size(""), None);
    }

    #[test]
    fn a_relative_path_cannot_look_like_a_sips_option() {
        assert_eq!(argument_path(Path::new("-Z.cr2")), Path::new("./-Z.cr2"));
        assert_eq!(argument_path(Path::new("a/b.psd")), Path::new("./a/b.psd"));
        assert_eq!(
            argument_path(Path::new("/x/-a.psd")),
            Path::new("/x/-a.psd")
        );
    }

    #[test]
    fn a_vector_image_fills_the_box_in_both_directions() {
        assert_eq!(enlarged_side((40, 20), (400, 400)), 400);
        assert_eq!(enlarged_side((612, 792), (3400, 2220)), 2220);
        assert_eq!(enlarged_side((4000, 2000), (100, 100)), 100);
    }

    #[test]
    fn the_covering_side_fits_both_orientations_and_never_enlarges() {
        // A landscape cell: the stored landscape size needs the longer side.
        assert_eq!(covering_side((6000, 4000), (306, 222)), Some(306));
        // A portrait box: the turned size (orientation 6) needs the longer side.
        assert_eq!(covering_side((6000, 4000), (222, 306)), Some(306));
        assert_eq!(covering_side((64, 32), (16, 16)), Some(16));
        // The image fits the box already: sips converts the full image.
        assert_eq!(covering_side((64, 32), (200, 200)), None);
        assert_eq!(covering_side((64, 32), (40, 200)), None);
    }

    #[test]
    fn a_psd_opens_in_the_grid_and_in_fullscreen() {
        let dir = tempfile::tempdir().unwrap();
        let path = sips_file(dir.path(), "psd");
        assert_eq!(dimensions(&path), Some((64, 32)));

        // The zoom decodes without a target: the full image.
        let zoom = decode_image_with_hint(&path, None, EmbeddedThumbnail::Deny).unwrap();
        assert_eq!(zoom.dimensions(), (64, 32));
        assert_red_left_blue_right(&zoom);
        // The fullscreen view and the grid get a small copy from sips itself:
        // `decode_image_with_hint` does not resize.
        for embedded in [EmbeddedThumbnail::Deny, EmbeddedThumbnail::Allow] {
            let small = decode_image_with_hint(&path, Some((32, 32)), embedded).unwrap();
            assert_eq!(small.dimensions(), (32, 16), "{embedded:?}");
            assert_red_left_blue_right(&small);
        }
        let fullscreen = load_and_resize(
            &path,
            Rect::new(0, 0, 4, 4),
            (8, 8),
            EmbeddedThumbnail::Deny,
        )
        .unwrap();
        assert_eq!(fullscreen.dimensions(), (32, 16));
        // A box larger than the image does not enlarge it.
        let large = decode(&path, Some((200, 200))).unwrap();
        assert_eq!(large.dimensions(), (64, 32));
    }

    #[test]
    fn a_16_bit_psd_opens_at_full_size_and_as_a_small_copy() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("source16.png");
        let wide = red_left_blue_right();
        ImageBuffer::<Rgb<u16>, _>::from_fn(64, 32, |x, y| {
            Rgb(wide.get_pixel(x, y).0.map(|c| u16::from(c) * 257))
        })
        .save(&png)
        .unwrap();
        let path = convert(&png, "psd");
        // sips writes a 16-bit TIFF at full size and an 8-bit TIFF for `-Z`.
        let full = decode(&path, None).unwrap();
        assert!(
            matches!(full, DynamicImage::ImageRgb16(_)),
            "{:?}",
            full.color()
        );
        assert_red_left_blue_right(&full);
        let small = decode(&path, Some((16, 16))).unwrap();
        assert_eq!(small.dimensions(), (16, 8));
        assert_red_left_blue_right(&small);
    }

    #[test]
    fn the_exif_orientation_of_a_sips_file_applies() {
        use image::ImageEncoder;
        let dir = tempfile::tempdir().unwrap();
        let jpeg = dir.path().join("rotated.jpg");
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(
            std::fs::File::create(&jpeg).unwrap(),
            95,
        );
        encoder
            .set_exif_metadata(EXIF_ORIENTATION_6.to_vec())
            .unwrap();
        encoder
            .write_image(
                red_left_blue_right().as_raw(),
                64,
                32,
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();
        let path = convert(&jpeg, "psd");
        // sips keeps the stored layout: the orientation comes through the TIFF Orientation tag.
        assert_eq!(dimensions(&path), Some((64, 32)));
        for target in [None, Some((16, 16))] {
            let rgba = decode(&path, target).unwrap().to_rgba8();
            let (w, h) = rgba.dimensions();
            assert!(h > w, "{target:?}: {w}x{h} is not upright");
            // Rotated 90 degrees clockwise: red on top, blue below.
            assert!(rgba.get_pixel(w / 2, h / 4).0[0] > 200, "{target:?}");
            assert!(rgba.get_pixel(w / 2, h * 3 / 4).0[2] > 200, "{target:?}");
        }
    }

    #[test]
    fn an_svg_fills_the_grid_and_the_fullscreen_box() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synthetic.svg");
        std::fs::write(
            &path,
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20">
                <rect width="20" height="20" fill="#ff0000"/>
                <rect x="20" width="20" height="20" fill="#0000ff"/></svg>"##,
        )
        .unwrap();
        assert_eq!(dimensions(&path), Some((40, 20)));
        let fullscreen = decode_image_with_hint(&path, Some((400, 400)), EmbeddedThumbnail::Deny);
        let fullscreen = fullscreen.unwrap();
        assert_eq!(fullscreen.dimensions(), (400, 200));
        assert_red_left_blue_right(&fullscreen);
        let grid = decode_image_with_hint(&path, Some((16, 16)), EmbeddedThumbnail::Allow);
        assert_eq!(grid.unwrap().dimensions(), (16, 8));
        let zoom = decode_image_with_hint(&path, None, EmbeddedThumbnail::Deny).unwrap();
        assert_eq!(zoom.dimensions(), (VECTOR_ZOOM_SIDE, VECTOR_ZOOM_SIDE / 2));
    }

    #[test]
    fn a_pdf_page_opens_on_white_paper() {
        let dir = tempfile::tempdir().unwrap();
        // Left half transparent, right half blue.
        let png = dir.path().join("page.png");
        image::RgbaImage::from_fn(64, 32, |x, _| {
            image::Rgba(if x < 32 {
                [0, 0, 0, 0]
            } else {
                [0, 0, 255, 255]
            })
        })
        .save(&png)
        .unwrap();
        let path = convert(&png, "pdf");
        let (w, h) = dimensions(&path).expect("the page size");
        assert_eq!(w, 2 * h, "{w}x{h}");
        let page = decode_image_with_hint(&path, Some((128, 128)), EmbeddedThumbnail::Deny)
            .unwrap()
            .to_rgba8();
        assert_eq!(page.dimensions(), (128, 64));
        assert_eq!(page.get_pixel(8, 32).0, [255, 255, 255, 255], "white paper");
        let blue = page.get_pixel(120, 32).0;
        assert!(blue[2] > 200 && blue[0] < 60 && blue[3] == 255, "{blue:?}");
    }

    #[test]
    fn a_file_that_sips_cannot_read_reports_the_sips_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.cr2");
        std::fs::write(&path, b"this is not a camera RAW file").unwrap();
        let error = decode(&path, None).unwrap_err().to_string();
        assert!(error.contains("sips failed"), "{error}");
        assert!(error.contains("Cannot extract image"), "{error}");
        assert!(decode(&path, Some((16, 16))).is_err());
        assert_eq!(dimensions(&path), None);
    }

    /// Decode a real camera RAW file: `VIEWR_TEST_RAW=/path/to/photo.cr2`. A portrait photo must
    /// come out upright.
    #[test]
    #[ignore = "needs VIEWR_TEST_RAW, a real camera RAW file"]
    fn decodes_raw_from_env() {
        decodes_real_file("VIEWR_TEST_RAW");
    }

    /// Decode a real Photoshop file: `VIEWR_TEST_PSD=/path/to/image.psd`.
    #[test]
    #[ignore = "needs VIEWR_TEST_PSD, a real Photoshop file"]
    fn decodes_psd_from_env() {
        decodes_real_file("VIEWR_TEST_PSD");
    }

    fn decodes_real_file(variable: &str) {
        let Some(path) = crate::decode::real_test_file(variable) else {
            return;
        };
        let path = path.as_path();
        let full =
            decode_image_with_hint(path, None, EmbeddedThumbnail::Deny).expect("full decode");
        let grid = decode_image_with_hint(path, Some((300, 300)), EmbeddedThumbnail::Allow)
            .expect("grid decode");
        // The grid copy covers the cell and has the shape of the full decode.
        assert!(grid.width() >= 300 || grid.height() >= 300 || grid.width() == full.width());
        assert_eq!(grid.width() >= grid.height(), full.width() >= full.height());
        let stored = dimensions(path).expect("dimensions");
        let rotated = (stored.1, stored.0);
        assert!(
            (full.width(), full.height()) == stored || (full.width(), full.height()) == rotated,
            "stored {stored:?}, decoded {}x{}",
            full.width(),
            full.height()
        );
    }
}
