//! Camera RAW, Photoshop, PDF, SVG and the other `ImageIO` formats through macOS `/usr/bin/sips`.
//!
//! sips writes an uncompressed TIFF into a temp file, and the `image` crate reads it. The TIFF
//! carries the EXIF orientation as its Orientation tag. `sips -Z` enlarges a small image too, so
//! the route reads the size first. The embedded JPEG of a RAW file corrects a develop that sips
//! does not turn or develops black.

use crate::decode::Decoded;
use crate::decode::embedded_jpeg::{self, EmbeddedJpeg};
use crate::decode::raster::{add_file_color, decode_upright};
use crate::decode::resize::scaled_down_size;
use crate::scanner;
use image::metadata::Orientation;
use image::{DynamicImage, GenericImageView, GrayImage, ImageFormat, ImageReader, Luma, Rgba};
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const SIPS: &str = "/usr/bin/sips";

/// sips draws the vector formats sharp at each size.
const VECTOR_EXTENSIONS: &[&str] = &["pdf", "svg"];

/// sips writes a vector image at 72 dpi by default: a letter page is only 612x792 px.
const VECTOR_ZOOM_SIDE: u32 = 4096;

/// Side of the gray pictures that decide a flip: 32x32 keeps the compare below 1 ms.
const COMPARE_SIDE: u32 = 32;

/// Samples per row and column of one gray cell.
const COMPARE_SAMPLES: u32 = 4;

/// The oriented candidate must come closer to the preview by this many gray levels (mean over
/// the picture). The tone of a develop differs from the preview in both candidates alike, so a
/// symmetric or flat picture gives a gap near 0 and keeps the develop as it is.
const FLIP_MARGIN: f32 = 2.0;

/// A covering embedded JPEG wins over sips. The zoom has no target and gets the full image from
/// sips, which the embedded JPEG corrects.
pub(crate) fn decode_with_preview(path: &Path, target: Option<(u32, u32)>) -> io::Result<Decoded> {
    let jpeg = embedded_jpeg::find(path);
    if let Some(image) = jpeg
        .as_ref()
        .zip(target)
        .and_then(|(jpeg, box_px)| embedded_jpeg::decode_covering(path, jpeg, box_px))
    {
        return Ok(image);
    }
    let developed = decode(path, target)?;
    Ok(match jpeg {
        Some(jpeg) => corrected(path, developed, &jpeg, target),
        None => developed,
    })
}

/// The embedded JPEG knows the orientation and shows a picture where sips develops black.
fn corrected(
    path: &Path,
    developed: Decoded,
    jpeg: &EmbeddedJpeg,
    target: Option<(u32, u32)>,
) -> Decoded {
    // A black PSD canvas is the real image. Only a RAW develop comes out black by mistake.
    if !scanner::has_extension(path, &["psd"]) && is_all_black(&developed.image) {
        // sips develops some RW2 files black, and a smaller picture beats a black one.
        return embedded_jpeg::decode(path, jpeg, target).unwrap_or(developed);
    }
    turned_like(path, developed, jpeg)
}

/// sips ignores the orientation of RAF and ARW files. A result with the turned aspect stays,
/// because sips turned it already. The aspect cannot show a flip or a square turn: the pixel
/// compare decides there.
fn turned_like(path: &Path, mut developed: Decoded, jpeg: &EmbeddedJpeg) -> Decoded {
    let size = (developed.image.width(), developed.image.height());
    let Some((exif, orientation)) = jpeg
        .orientation
        .and_then(|exif| Some((exif, Orientation::from_exif(exif)?)))
    else {
        return developed;
    };
    if preview_turns(size, jpeg)
        || (scanner::has_extension(path, &["raf", "arw"])
            && aspect_is_blind(size, (jpeg.width, jpeg.height), exif)
            && flip_gap(path, &developed.image, jpeg, orientation)
                .is_some_and(|gap| gap > FLIP_MARGIN))
    {
        developed.image.apply_orientation(orientation);
    }
    developed
}

/// True when the aspect cannot tell whether sips applied the orientation.
fn aspect_is_blind(developed: (u32, u32), stored: (u32, u32), orientation: u8) -> bool {
    let square = |(width, height): (u32, u32)| width == height;
    (2..=4).contains(&orientation)
        || ((5..=8).contains(&orientation) && (square(developed) || square(stored)))
}

/// How much closer the oriented develop is to the small reference picture than the develop as
/// it is, in mean gray levels. `None` when the file has no usable reference.
fn flip_gap(
    path: &Path,
    developed: &DynamicImage,
    jpeg: &EmbeddedJpeg,
    orientation: Orientation,
) -> Option<f32> {
    let reference = embedded_jpeg::decode_reference(path, jpeg, (COMPARE_SIDE, COMPARE_SIDE))?;
    let reference = gray_thumbnail(&reference.image);
    let unoriented = gray_thumbnail(developed);
    let mut oriented = DynamicImage::ImageLuma8(unoriented.clone());
    oriented.apply_orientation(orientation);
    let oriented = oriented.into_luma8();
    Some(mean_difference(&unoriented, &reference) - mean_difference(&oriented, &reference))
}

/// A gray picture of `COMPARE_SIDE` squared pixels. It averages a grid of samples per cell
/// instead of a resize, because a resize of 40 MP reads every pixel.
fn gray_thumbnail(image: &DynamicImage) -> GrayImage {
    let samples = COMPARE_SIDE * COMPARE_SAMPLES;
    let at = |index: u32, length: u32| (index * 2 + 1) * length / (samples * 2);
    GrayImage::from_fn(COMPARE_SIDE, COMPARE_SIDE, |cell_x, cell_y| {
        let mut sum = 0_u32;
        for sample_y in 0..COMPARE_SAMPLES {
            for sample_x in 0..COMPARE_SAMPLES {
                let x = at(cell_x * COMPARE_SAMPLES + sample_x, image.width());
                let y = at(cell_y * COMPARE_SAMPLES + sample_y, image.height());
                let [red, green, blue, _] = image.get_pixel(x, y).0;
                sum +=
                    (299 * u32::from(red) + 587 * u32::from(green) + 114 * u32::from(blue)) / 1000;
            }
        }
        Luma([u8::try_from(sum / (COMPARE_SAMPLES * COMPARE_SAMPLES)).unwrap_or(u8::MAX)])
    })
}

/// The mean absolute difference of two gray pictures of the same size, in gray levels.
fn mean_difference(left: &GrayImage, right: &GrayImage) -> f32 {
    let total: u64 = left
        .as_raw()
        .iter()
        .zip(right.as_raw())
        .map(|(&a, &b)| u64::from(a.abs_diff(b)))
        .sum();
    total as f32 / left.as_raw().len() as f32
}

/// True when `orientation` turns by 90 or 270 degrees and `developed` keeps the aspect of the
/// stored preview. A square size tells nothing about the turn.
fn misses_turn(developed: (u32, u32), stored: (u32, u32), orientation: u8) -> bool {
    let landscape = |(width, height): (u32, u32)| (width != height).then_some(width > height);
    (5..=8).contains(&orientation)
        && landscape(developed).is_some_and(|wide| landscape(stored) == Some(wide))
}

/// True when every color sample is 0. The alpha samples do not count.
fn is_all_black(image: &DynamicImage) -> bool {
    let color = image.color();
    let pixel = usize::from(color.bytes_per_pixel());
    let channels = usize::from(color.channel_count());
    let samples = if color.has_alpha() {
        pixel - pixel / channels
    } else {
        pixel
    };
    image
        .as_bytes()
        .chunks_exact(pixel)
        .all(|bytes| bytes.iter().take(samples).all(|&byte| byte == 0))
}

/// `target` is the grid or fullscreen box, and sips writes a copy that covers it. The zoom has
/// no target and gets the full image.
pub(crate) fn decode(path: &Path, target: Option<(u32, u32)>) -> io::Result<Decoded> {
    let path = &argument_path(path);
    let max_side = requested_side(path, target)?.map(|side| side.to_string());
    let tiff = tempfile::Builder::new()
        .prefix("phewr-sips-")
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
    let mut decoded =
        decode_upright(reader).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    add_file_color(&mut decoded, tiff.path(), Some(ImageFormat::Tiff));
    if scanner::has_extension(path, &["pdf"]) {
        decoded.image = on_white(&decoded.image);
    }
    Ok(decoded)
}

/// sips reads an argument that starts with `-` as a function name (`phewr -- -x.cr2`). A
/// relative path gets `./` in front.
fn argument_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        Path::new(".").join(path)
    }
}

/// `None` for the full image. A raster zoom needs no size query.
fn requested_side(path: &Path, target: Option<(u32, u32)>) -> io::Result<Option<u32>> {
    let vector = scanner::has_extension(path, VECTOR_EXTENSIONS);
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

/// The route does not know the EXIF orientation. The side covers the stored and the turned
/// size. `None` when the full image is not larger than that side.
fn covering_side(stored: (u32, u32), target: (u32, u32)) -> Option<u32> {
    let longest = |(w, h): (u32, u32)| w.max(h);
    let fitted = |size: (u32, u32)| scaled_down_size(size, target).map_or(longest(size), longest);
    let side = fitted(stored).max(fitted((stored.1, stored.0)));
    (side < longest(stored)).then_some(side)
}

/// The longest side that fills `target`, larger or smaller than the vector image.
fn enlarged_side(size: (u32, u32), target: (u32, u32)) -> u32 {
    let (w, h) = (f64::from(size.0.max(1)), f64::from(size.1.max(1)));
    let scale = f64::min(f64::from(target.0) / w, f64::from(target.1) / h);
    (w.max(h) * scale).round().max(1.0) as u32
}

/// Black text on a transparent page needs white paper on a dark terminal.
fn on_white(image: &DynamicImage) -> DynamicImage {
    let mut page = image.to_rgba8();
    for Rgba([r, g, b, a]) in page.pixels_mut() {
        let alpha = u16::from(*a);
        let blend = |c: u8| ((u16::from(c) * alpha + 255 * (255 - alpha) + 127) / 255) as u8;
        (*r, *g, *b, *a) = (blend(*r), blend(*g), blend(*b), 255);
    }
    DynamicImage::ImageRgba8(page)
}

pub(crate) fn dimensions(path: &Path) -> Option<(u32, u32)> {
    stored_size(&argument_path(path)).ok()
}

/// The size of the fullscreen view. sips reports the stored size, before the orientation.
pub(crate) fn shown_dimensions(path: &Path) -> Option<(u32, u32)> {
    let size = dimensions(path)?;
    let turns = match embedded_jpeg::find(path) {
        Some(jpeg) => preview_turns(size, &jpeg),
        // sips turns the develop of an ORF by its IFD0 orientation.
        None => embedded_jpeg::container_orientation(path).is_some_and(|o| (5..=8).contains(&o)),
    };
    Some(if turns { (size.1, size.0) } else { size })
}

/// True when sips missed the quarter turn that the embedded JPEG knows.
fn preview_turns(size: (u32, u32), jpeg: &EmbeddedJpeg) -> bool {
    jpeg.orientation
        .is_some_and(|orientation| misses_turn(size, (jpeg.width, jpeg.height), orientation))
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

/// A PDF and an SVG have sizes in points with decimals (`pixelWidth: 612.000`). They round up.
/// sips writes `<nil>` for a file that it cannot read.
fn parse_size(stdout: &str) -> Option<(u32, u32)> {
    let value = |key: &str| {
        stdout.lines().find_map(|line| {
            let (name, value) = line.trim().split_once(':')?;
            (name == key).then(|| pixels(value.trim()))?
        })
    };
    Some((value("pixelWidth")?, value("pixelHeight")?))
}

/// `None` for `<nil>`, zero, a negative or a huge value.
fn pixels(value: &str) -> Option<u32> {
    let value = value.parse::<f64>().ok()?.ceil();
    (value >= 1.0 && value <= f64::from(u32::MAX)).then_some(value as u32)
}

/// A failure returns the exit status and stderr.
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
        FLIP_MARGIN, SIPS, VECTOR_ZOOM_SIDE, argument_path, corrected, covering_side, decode,
        dimensions, enlarged_side, flip_gap, gray_thumbnail, is_all_black, mean_difference,
        misses_turn, parse_size, preview_turns,
    };
    use crate::decode::color::SourceColor;
    use crate::decode::embedded_jpeg::find;
    use crate::decode::resize::load_and_resize_px;
    use crate::decode::{Decoded, EmbeddedThumbnail, decode_image_with_hint};
    use crate::info;
    use crate::test_support::{
        encode_jpeg, quadrant_image, test_dir, write_psd, write_raf_with_thumbnail, write_tiff_raw,
        write_tiff_raw_with_jpeg,
    };
    use image::metadata::Orientation;
    use image::{
        DynamicImage, GenericImageView, ImageBuffer, LumaA, Rgb, RgbImage, Rgba, RgbaImage,
    };
    use std::path::{Path, PathBuf};
    use std::process::Command;

    /// A little-endian TIFF block with one IFD entry: Orientation = 6 (rotate 90 CW).
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

    fn sips_file(dir: &Path, format: &str) -> PathBuf {
        let png = dir.join("source.png");
        red_left_blue_right().save(&png).unwrap();
        convert(&png, format)
    }

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
    fn size_comes_from_sips_output() {
        let stdout = "/tmp/a.psd\n  pixelWidth: 794\n  pixelHeight: 374\n";
        assert_eq!(parse_size(stdout), Some((794, 374)));
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
    fn relative_path_is_no_sips_option() {
        assert_eq!(argument_path(Path::new("-Z.cr2")), Path::new("./-Z.cr2"));
        assert_eq!(argument_path(Path::new("a/b.psd")), Path::new("./a/b.psd"));
        assert_eq!(
            argument_path(Path::new("/x/-a.psd")),
            Path::new("/x/-a.psd")
        );
    }

    #[test]
    fn vector_image_fills_box() {
        assert_eq!(enlarged_side((40, 20), (400, 400)), 400);
        assert_eq!(enlarged_side((612, 792), (3400, 2220)), 2220);
        assert_eq!(enlarged_side((4000, 2000), (100, 100)), 100);
    }

    #[test]
    fn covering_side_fits_both_orientations() {
        // The stored landscape size needs the longer side.
        assert_eq!(covering_side((6000, 4000), (306, 222)), Some(306));
        // The turned size (orientation 6) needs the longer side.
        assert_eq!(covering_side((6000, 4000), (222, 306)), Some(306));
        assert_eq!(covering_side((64, 32), (16, 16)), Some(16));
        assert_eq!(covering_side((64, 32), (200, 200)), None);
        assert_eq!(covering_side((64, 32), (40, 200)), None);
    }

    #[test]
    fn psd_opens_in_grid_and_fullscreen() {
        let dir = tempfile::tempdir().unwrap();
        let path = sips_file(dir.path(), "psd");
        assert_eq!(dimensions(&path), Some((64, 32)));

        let zoom = decode_image_with_hint(&path, None, EmbeddedThumbnail::Deny).unwrap();
        assert_eq!(zoom.dimensions(), (64, 32));
        assert_red_left_blue_right(&zoom);
        // The fullscreen view and the grid get a small copy from sips itself.
        for embedded in [EmbeddedThumbnail::Deny, EmbeddedThumbnail::Allow] {
            let small = decode_image_with_hint(&path, Some((32, 32)), embedded).unwrap();
            assert_eq!(small.dimensions(), (32, 16), "{embedded:?}");
            assert_red_left_blue_right(&small);
        }
        let fullscreen = load_and_resize_px(&path, (32, 32), EmbeddedThumbnail::Deny).unwrap();
        assert_eq!(fullscreen.dimensions(), (32, 16));
        let large = decode(&path, Some((200, 200))).unwrap().image;
        assert_eq!(large.dimensions(), (64, 32));
    }

    #[test]
    fn sixteen_bit_psd_opens_both_sizes() {
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
        let full = decode(&path, None).unwrap().image;
        assert!(
            matches!(full, DynamicImage::ImageRgb16(_)),
            "{:?}",
            full.color()
        );
        assert_red_left_blue_right(&full);
        let small = decode(&path, Some((16, 16))).unwrap().image;
        assert_eq!(small.dimensions(), (16, 8));
        assert_red_left_blue_right(&small);
    }

    #[test]
    fn sips_exif_orientation_applies() {
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
        // sips keeps the stored layout. The orientation comes through the TIFF Orientation tag.
        assert_eq!(dimensions(&path), Some((64, 32)));
        for target in [None, Some((16, 16))] {
            let rgba = decode(&path, target).unwrap().image.to_rgba8();
            let (w, h) = rgba.dimensions();
            assert!(h > w, "{target:?}: {w}x{h} is not upright");
            assert!(rgba.get_pixel(w / 2, h / 4).0[0] > 200, "{target:?}");
            assert!(rgba.get_pixel(w / 2, h * 3 / 4).0[2] > 200, "{target:?}");
        }
    }

    #[test]
    fn svg_fills_grid_and_fullscreen() {
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
    fn pdf_page_opens_on_white() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("page.png");
        RgbaImage::from_fn(64, 32, |x, _| {
            Rgba(if x < 32 {
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
    fn unreadable_file_reports_sips_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.cr2");
        std::fs::write(&path, b"this is not a camera RAW file").unwrap();
        let error = decode(&path, None).unwrap_err().to_string();
        assert!(error.contains("sips failed"), "{error}");
        assert!(error.contains("Cannot extract image"), "{error}");
        assert!(decode(&path, Some((16, 16))).is_err());
        assert_eq!(dimensions(&path), None);
    }

    /// A sips develop of the synthetic RAW, which sips itself cannot read.
    fn developed(image: DynamicImage) -> Decoded {
        Decoded {
            image,
            color: SourceColor::Srgb,
            opaque: true,
            peak: None,
        }
    }

    #[test]
    fn turn_decision_follows_aspect() {
        // sips ignored the orientation: the develop keeps the aspect of the stored preview.
        assert!(misses_turn((7728, 5152), (1920, 1280), 6));
        assert!(misses_turn((5152, 7728), (1280, 1920), 8));
        assert!(misses_turn((7728, 5152), (1920, 1280), 5));
        // sips turned it already.
        assert!(!misses_turn((5152, 7728), (1920, 1280), 6));
        assert!(!misses_turn((5152, 7728), (1920, 1280), 8));
        // The orientation does not turn by 90 or 270 degrees.
        assert!(!misses_turn((7728, 5152), (1920, 1280), 1));
        assert!(!misses_turn((7728, 5152), (1920, 1280), 3));
        // A square size tells nothing.
        assert!(!misses_turn((4000, 4000), (1920, 1280), 6));
        assert!(!misses_turn((7728, 5152), (1600, 1600), 6));
    }

    #[test]
    fn unturned_develop_turns_once() {
        let dir = test_dir();
        let path = dir.path().join("photo.nef");
        write_tiff_raw(&path, 6);
        let jpeg = find(&path).unwrap();
        let unturned = DynamicImage::ImageRgb8(red_left_blue_right());
        let shown = corrected(&path, developed(unturned.clone()), &jpeg, None).image;
        // Orientation 6 turns the stored image 90 degrees clockwise, and red goes to the top.
        assert_eq!(shown.dimensions(), (32, 64));
        let rgba = shown.to_rgba8();
        assert_eq!(rgba.get_pixel(16, 8).0, [255, 0, 0, 255]);
        assert_eq!(rgba.get_pixel(16, 56).0, [0, 0, 255, 255]);
        let turned = unturned.rotate90();
        let kept = corrected(&path, developed(turned.clone()), &jpeg, None).image;
        assert_eq!(kept, turned);
    }

    /// The preview oriented the way the RAW file asks for.
    fn upright(stored: &RgbImage, orientation: u32) -> DynamicImage {
        let mut image = DynamicImage::from(stored.clone());
        image
            .apply_orientation(Orientation::from_exif(u8::try_from(orientation).unwrap()).unwrap());
        image
    }

    /// Both RAW kinds that sips develops without the orientation, with an EXIF thumbnail.
    fn raw_files(dir: &Path, picture: &RgbImage, orientation: u32) -> [PathBuf; 2] {
        let (width, height) = picture.dimensions();
        let jpeg = encode_jpeg(picture);
        let thumbnail = encode_jpeg(&image::imageops::resize(
            picture,
            width * 3 / 4,
            height,
            image::imageops::FilterType::Nearest,
        ));
        let raf = dir.join("photo.raf");
        write_raf_with_thumbnail(&raf, &jpeg, orientation, &thumbnail);
        let arw = dir.join("photo.arw");
        write_tiff_raw_with_jpeg(&arw, &jpeg, orientation, Some(&thumbnail));
        [raf, arw]
    }

    fn flip_files(dir: &Path, size: (u32, u32), orientation: u32) -> [PathBuf; 2] {
        raw_files(dir, &quadrant_image(size.0, size.1), orientation)
    }

    /// 30 gray levels brighter than `picture`, as a develop is against the camera JPEG.
    fn brighter(picture: &RgbImage) -> RgbImage {
        let mut brighter = picture.clone();
        for pixel in brighter.pixels_mut() {
            pixel.0 = pixel.0.map(|level| level.saturating_add(30));
        }
        brighter
    }

    #[test]
    fn unflipped_develop_flips_for_orientation_2_3_4() {
        for orientation in [2, 3, 4] {
            let dir = test_dir();
            let stored = quadrant_image(64, 32);
            let want = upright(&stored, orientation);
            for path in flip_files(dir.path(), (64, 32), orientation) {
                let jpeg = find(&path).unwrap();
                let unflipped = DynamicImage::from(stored.clone());
                let shown = corrected(&path, developed(unflipped.clone()), &jpeg, None).image;
                assert_eq!(shown, want, "{} orientation {orientation}", path.display());
                // A develop that is correct already stays.
                let kept = corrected(&path, developed(want.clone()), &jpeg, None).image;
                assert_eq!(kept, want, "{} orientation {orientation}", path.display());
            }
        }
    }

    #[test]
    fn brighter_and_cropped_develop_still_flips() {
        let dir = test_dir();
        let stored = quadrant_image(64, 32);
        // The develop is brighter and misses 2 pixels of each edge of the preview.
        let tone = brighter(&stored);
        let cropped = DynamicImage::from(tone).crop_imm(2, 2, 60, 28).to_rgb8();
        for orientation in [2, 3, 4] {
            let mut want = DynamicImage::from(cropped.clone());
            want.apply_orientation(Orientation::from_exif(orientation).unwrap());
            for path in flip_files(dir.path(), (64, 32), u32::from(orientation)) {
                let jpeg = find(&path).unwrap();
                let shown = corrected(&path, developed(cropped.clone().into()), &jpeg, None).image;
                assert_eq!(shown, want, "{} orientation {orientation}", path.display());
                let kept = corrected(&path, developed(want.clone()), &jpeg, None).image;
                assert_eq!(kept, want, "{} orientation {orientation}", path.display());
            }
        }
    }

    #[test]
    fn square_develop_turns_by_pixels() {
        let dir = test_dir();
        let stored = quadrant_image(64, 64);
        let want = upright(&stored, 6);
        for path in flip_files(dir.path(), (64, 64), 6) {
            let jpeg = find(&path).unwrap();
            let shown = corrected(&path, developed(stored.clone().into()), &jpeg, None).image;
            assert_eq!(shown, want);
            let kept = corrected(&path, developed(want.clone()), &jpeg, None).image;
            assert_eq!(kept, want);
        }
    }

    #[test]
    fn symmetric_picture_stays() {
        let dir = test_dir();
        let flat = RgbImage::from_pixel(64, 32, Rgb([200, 30, 30]));
        // Symmetric left to right, so the horizontal flip changes nothing.
        let stripes = RgbImage::from_fn(64, 32, |x, y| {
            let level = if (16..48).contains(&x) { 200 } else { 20 };
            Rgb([level, level, level + u8::try_from(y).unwrap()])
        });
        for (picture, orientations) in [(flat, vec![2, 3, 4]), (stripes, vec![2])] {
            for orientation in orientations {
                // The develop is brighter than the preview, as a real develop is.
                let develop = DynamicImage::from(brighter(&picture));
                for path in raw_files(dir.path(), &picture, orientation) {
                    let jpeg = find(&path).unwrap();
                    let shown = corrected(&path, developed(develop.clone()), &jpeg, None).image;
                    assert_eq!(
                        shown,
                        develop,
                        "{} orientation {orientation}",
                        path.display()
                    );
                }
            }
        }
    }

    #[test]
    fn black_develop_takes_preview() {
        let dir = test_dir();
        let path = dir.path().join("photo.rw2");
        write_tiff_raw(&path, 1);
        let jpeg = find(&path).unwrap();
        let black = RgbaImage::from_pixel(64, 32, Rgba([0, 0, 0, 255]));
        let shown = corrected(&path, developed(black.into()), &jpeg, Some((64, 32))).image;
        assert_eq!(shown.dimensions(), (64, 32));
        assert_red_left_blue_right(&shown);
    }

    #[test]
    fn black_psd_stays_black() {
        let dir = test_dir();
        let path = dir.path().join("canvas.psd");
        write_psd(&path, true);
        let jpeg = find(&path).unwrap();
        let black = RgbaImage::from_pixel(64, 32, Rgba([0, 0, 0, 255]));
        let shown = corrected(&path, developed(black.into()), &jpeg, Some((64, 32))).image;
        assert!(is_all_black(&shown));
    }

    #[test]
    fn info_size_follows_turn() {
        let dir = test_dir();
        let path = dir.path().join("photo.arw");
        write_tiff_raw(&path, 6);
        let turned = find(&path).unwrap();
        assert!(preview_turns((7728, 5152), &turned));
        // A size that sips reports turned stays.
        assert!(!preview_turns((5152, 7728), &turned));
        write_tiff_raw(&path, 1);
        let upright = find(&path).unwrap();
        assert!(!preview_turns((7728, 5152), &upright));
    }

    #[test]
    fn black_check_ignores_alpha() {
        let rgba = |pixel: [u8; 4]| DynamicImage::from(RgbaImage::from_pixel(4, 2, Rgba(pixel)));
        assert!(is_all_black(&rgba([0, 0, 0, 255])));
        assert!(is_all_black(&rgba([0, 0, 0, 0])));
        assert!(!is_all_black(&rgba([0, 0, 1, 255])));
        let mut one_red = RgbImage::new(4, 2);
        assert!(is_all_black(&one_red.clone().into()));
        one_red.put_pixel(3, 1, Rgb([1, 0, 0]));
        assert!(!is_all_black(&one_red.into()));
        let gray = ImageBuffer::from_pixel(4, 2, LumaA([0_u16, u16::MAX]));
        assert!(is_all_black(&DynamicImage::ImageLumaA16(gray)));
    }

    /// `PHEWR_TEST_RAW=/path/to/photo.cr2`. A portrait photo must come out upright and not black.
    #[test]
    #[ignore = "needs PHEWR_TEST_RAW, a real camera RAW file"]
    fn decodes_raw_from_env() {
        decodes_real_file("PHEWR_TEST_RAW");
    }

    /// `PHEWR_TEST_PSD=/path/to/image.psd`.
    #[test]
    #[ignore = "needs PHEWR_TEST_PSD, a real Photoshop file"]
    fn decodes_psd_from_env() {
        decodes_real_file("PHEWR_TEST_PSD");
    }

    /// `PHEWR_TEST_RAW=/path/to/photo.raf` or `.arw`, upright and unflipped in the file. A copy
    /// with the orientation 2, 3 and 4 written by exiftool must show flipped like the original.
    #[test]
    #[ignore = "needs PHEWR_TEST_RAW, a real RAF or ARW file, and exiftool"]
    fn real_flipped_copy_shows_upright() {
        let Some(path) = crate::decode::real_test_file("PHEWR_TEST_RAW") else {
            return;
        };
        if !crate::scanner::has_extension(&path, &["raf", "arw"]) {
            eprintln!("skipped: {} is no RAF or ARW file", path.display());
            return;
        }
        let dir = test_dir();
        let decode_full = |file: &Path| {
            decode_image_with_hint(file, None, EmbeddedThumbnail::Deny).expect("full decode")
        };
        let original = decode_full(&path);
        for orientation in [2_u8, 3, 4] {
            let copy = dir.path().join(path.file_name().unwrap());
            std::fs::copy(&path, &copy).unwrap();
            let status = Command::new("exiftool")
                .arg(format!("-Orientation#={orientation}"))
                .arg("-overwrite_original")
                .arg(&copy)
                .output()
                .expect("exiftool")
                .status;
            assert!(status.success(), "exiftool cannot write the orientation");
            let mut want = original.clone();
            want.apply_orientation(Orientation::from_exif(orientation).unwrap());
            assert_eq!(decode_full(&copy), want, "orientation {orientation}");
            // The gap that decides, and what the whole compare costs on this file.
            let jpeg = find(&copy).unwrap();
            let rule = Orientation::from_exif(orientation).unwrap();
            let sips = decode(&copy, None).expect("sips develop").image;
            let start = std::time::Instant::now();
            let gap = flip_gap(&copy, &sips, &jpeg, rule).expect("reference");
            let spent = start.elapsed();
            let mut turned = sips.clone();
            turned.apply_orientation(rule);
            let kept = flip_gap(&copy, &turned, &jpeg, rule).expect("reference");
            eprintln!(
                "{} orientation {orientation}: gap {gap:.2} (unoriented develop), {kept:.2} (oriented develop), {spent:?}",
                copy.file_name().unwrap().to_string_lossy()
            );
        }
    }

    /// `PHEWR_TEST_ORF=/path/to/photo.orf`. Copies with the IFD0 orientation 1 and 6, written by
    /// exiftool, give the maker note preview, turned like the sips develop.
    #[test]
    #[ignore = "needs PHEWR_TEST_ORF, a real ORF file, and exiftool"]
    fn real_orf_preview_turns_like_develop() {
        let Some(path) = crate::decode::real_test_file("PHEWR_TEST_ORF") else {
            return;
        };
        let dir = test_dir();
        let grid_box = (624, 468);
        for orientation in [1_u8, 6] {
            let copy = dir.path().join(format!("orientation-{orientation}.orf"));
            std::fs::copy(&path, &copy).unwrap();
            let status = Command::new("exiftool")
                .arg(format!("-IFD0:Orientation#={orientation}"))
                .arg("-overwrite_original")
                .arg(&copy)
                .output()
                .expect("exiftool")
                .status;
            assert!(status.success(), "exiftool cannot write the orientation");
            let start = std::time::Instant::now();
            let jpeg = find(&copy).expect("the maker note preview");
            let spent = start.elapsed();
            assert_eq!(jpeg.orientation, Some(orientation));
            assert!(jpeg.width.max(jpeg.height) >= 1024, "{jpeg:?}");
            let turned = orientation == 6;
            let (width, height) = jpeg.shown_size();
            assert_eq!(width < height, turned, "shown size {width}x{height}");
            let preview = crate::decode::embedded_jpeg::decode(&copy, &jpeg, Some(grid_box))
                .expect("preview decode")
                .image;
            assert_eq!(preview.width() < preview.height(), turned);
            let develop = decode(&copy, Some(grid_box)).expect("sips develop").image;
            let reference = gray_thumbnail(&develop);
            let mut upside_down = preview.clone();
            upside_down.apply_orientation(Orientation::Rotate180);
            let same = mean_difference(&gray_thumbnail(&preview), &reference);
            let flipped = mean_difference(&gray_thumbnail(&upside_down), &reference);
            assert!(
                same + FLIP_MARGIN < flipped,
                "{same:.1} against {flipped:.1}"
            );
            eprintln!(
                "orientation {orientation}: preview {}x{} found in {spent:?}, grid decode {}x{}, \
                 gray difference to the develop {same:.1}, upside down {flipped:.1}",
                jpeg.width,
                jpeg.height,
                preview.width(),
                preview.height()
            );
        }
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
        let fullscreen = decode_image_with_hint(path, Some((3078, 2052)), EmbeddedThumbnail::Deny)
            .expect("fullscreen decode");
        assert!(grid.width() >= 300 || grid.height() >= 300 || grid.width() == full.width());
        let landscape = |image: &DynamicImage| image.width() >= image.height();
        assert_eq!(landscape(&grid), landscape(&full), "grid against full");
        assert_eq!(
            landscape(&fullscreen),
            landscape(&full),
            "fullscreen against full"
        );
        for (name, image) in [
            ("grid", &grid),
            ("fullscreen", &fullscreen),
            ("full", &full),
        ] {
            assert!(!is_all_black(image), "{name} decode is black");
        }
        let stored = dimensions(path).expect("dimensions");
        let rotated = (stored.1, stored.0);
        // Only a black develop gives way to the embedded JPEG.
        let preview = find(path).map(|jpeg| jpeg.shown_size());
        let black_develop = || is_all_black(&decode(path, None).expect("sips develop").image);
        let size = (full.width(), full.height());
        assert!(
            size == stored || size == rotated || (Some(size) == preview && black_develop()),
            "stored {stored:?}, decoded {}x{}",
            full.width(),
            full.height()
        );
        let panel = info::dimensions(path).expect("info panel size");
        assert_eq!(
            panel.0 >= panel.1,
            landscape(&full),
            "info panel {panel:?} against full"
        );
    }
}
