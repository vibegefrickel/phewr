//! File and EXIF metadata for the info panel.

use crate::decode::{self, Route};
use chrono::{DateTime, Local};
use exif::{In, Tag};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

/// An empty label marks a section heading.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ImageInfo {
    pub(crate) rows: Vec<(String, String)>,
}

pub(crate) fn load(path: &Path) -> ImageInfo {
    let mut rows: Vec<(String, String)> = Vec::new();

    rows.push((String::new(), "File".to_owned()));
    rows.push((
        "Name".to_owned(),
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    ));
    if let Some(parent) = path.parent() {
        let folder = std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
        rows.push(("Folder".to_owned(), folder.display().to_string()));
    }
    if let Ok(metadata) = std::fs::metadata(path) {
        rows.push(("Size".to_owned(), human_size(metadata.len())));
        if let Ok(modified) = metadata.modified() {
            let local: DateTime<Local> = modified.into();
            rows.push((
                "Modified".to_owned(),
                local.format("%Y-%m-%d %H:%M:%S").to_string(),
            ));
        }
    }
    if let Some((w, h)) = dimensions(path) {
        rows.push((
            "Dimensions".to_owned(),
            format!("{w} x {h} ({})", megapixels(w, h)),
        ));
    }
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        rows.push(("Format".to_owned(), ext.to_ascii_uppercase()));
    }

    let exif_rows = exif_rows(path);
    if !exif_rows.is_empty() {
        rows.push((String::new(), "EXIF".to_owned()));
        rows.extend(exif_rows);
    }

    ImageInfo { rows }
}

pub(crate) fn megapixels(w: u32, h: u32) -> String {
    let mp = (f64::from(w) * f64::from(h)) / 1_000_000.0;
    if mp >= 10.0 {
        format!("{mp:.0} MP")
    } else {
        format!("{mp:.1} MP")
    }
}

pub(crate) fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub(crate) fn dimensions(path: &Path) -> Option<(u32, u32)> {
    #[cfg(feature = "heic")]
    if decode::libheif::decodes_with_libheif(path) {
        return decode::libheif::libheif_dimensions(path);
    }
    match Route::for_path(path) {
        Route::Raster => decode::raster::header_dimensions(path),
        Route::Jxl => decode::jxl::dimensions(path),
        #[cfg(all(feature = "sips", target_os = "macos"))]
        Route::Sips => decode::sips::shown_dimensions(path),
    }
}

fn exif_rows(path: &Path) -> Vec<(String, String)> {
    let Some(exif) = read_exif(path) else {
        return Vec::new();
    };
    let mut rows = Vec::new();

    let value = |tag: Tag| -> Option<String> {
        exif.get_field(tag, In::PRIMARY).map(|field| {
            field
                .display_value()
                .with_unit(&exif)
                .to_string()
                .trim_matches('"')
                .trim()
                .to_owned()
        })
    };

    let make = value(Tag::Make);
    let model = value(Tag::Model);
    let camera = match (make, model) {
        (Some(make), Some(model)) if model.starts_with(&make) => model,
        (Some(make), Some(model)) => format!("{make} {model}"),
        (Some(make), None) => make,
        (None, Some(model)) => model,
        (None, None) => String::new(),
    };
    if !camera.is_empty() {
        rows.push(("Camera".to_owned(), camera));
    }
    if let Some(lens) = value(Tag::LensModel) {
        rows.push(("Lens".to_owned(), lens));
    }
    if let Some(taken) = value(Tag::DateTimeOriginal).or_else(|| value(Tag::DateTime)) {
        rows.push(("Taken".to_owned(), taken));
    }
    if let Some(exposure) = value(Tag::ExposureTime) {
        rows.push(("Exposure".to_owned(), exposure));
    }
    if let Some(aperture) = value(Tag::FNumber) {
        rows.push(("Aperture".to_owned(), aperture));
    }
    if let Some(iso) = value(Tag::PhotographicSensitivity) {
        rows.push(("ISO".to_owned(), iso));
    }
    let focal = value(Tag::FocalLength);
    let focal_35 = value(Tag::FocalLengthIn35mmFilm);
    match (focal, focal_35) {
        (Some(f), Some(f35)) => rows.push(("Focal length".to_owned(), format!("{f} ({f35} eq.)"))),
        (Some(f), None) => rows.push(("Focal length".to_owned(), f)),
        (None, Some(f35)) => rows.push(("Focal length".to_owned(), format!("{f35} eq."))),
        (None, None) => {}
    }
    if let Some(flash) = value(Tag::Flash) {
        rows.push(("Flash".to_owned(), flash));
    }
    if let Some(orientation) = value(Tag::Orientation) {
        rows.push(("Orientation".to_owned(), orientation));
    }
    if let Some(gps) = gps_coordinates(&exif) {
        rows.push(("GPS".to_owned(), gps));
    }
    if let Some(software) = value(Tag::Software) {
        rows.push(("Software".to_owned(), software));
    }
    if let Some(description) = value(Tag::ImageDescription)
        && !description.is_empty()
    {
        rows.push(("Description".to_owned(), description));
    }
    rows
}

/// `DateTimeOriginal`, else `DateTime`, as `YYYY-MM-DD HH:MM`.
pub(crate) fn date_taken(path: &Path) -> Option<String> {
    let exif = read_exif(path)?;
    let field = exif
        .get_field(Tag::DateTimeOriginal, In::PRIMARY)
        .or_else(|| exif.get_field(Tag::DateTime, In::PRIMARY))?;
    let exif::Value::Ascii(values) = &field.value else {
        return None;
    };
    let date = exif::DateTime::from_ascii(values.first()?).ok()?;
    Some(format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        date.year, date.month, date.day, date.hour, date.minute
    ))
}

/// JPEG, TIFF (with the TIFF-based RAW formats), HEIF, PNG, WebP and JPEG XL.
fn read_exif(path: &Path) -> Option<exif::Exif> {
    if Route::for_path(path) == Route::Jxl {
        return exif::Reader::new().read_raw(decode::jxl::exif(path)?).ok();
    }
    let file = File::open(path).ok()?;
    exif::Reader::new()
        .read_from_container(&mut BufReader::new(file))
        .ok()
}

/// The orientation (1 to 8) of JPEG bytes in memory. libjpeg-turbo ignores it.
#[cfg(any(feature = "turbo", all(feature = "sips", target_os = "macos")))]
pub(crate) fn orientation_from_bytes(data: &[u8]) -> Option<u8> {
    let exif = exif::Reader::new()
        .read_from_container(&mut std::io::Cursor::new(data))
        .ok()?;
    let field = exif.get_field(Tag::Orientation, In::PRIMARY)?;
    let orientation = u8::try_from(field.value.get_uint(0)?).ok()?;
    (1..=8).contains(&orientation).then_some(orientation)
}

fn gps_coordinates(exif: &exif::Exif) -> Option<String> {
    let lat = gps_decimal(exif, Tag::GPSLatitude, Tag::GPSLatitudeRef, 'S')?;
    let lon = gps_decimal(exif, Tag::GPSLongitude, Tag::GPSLongitudeRef, 'W')?;
    Some(format!("{lat:.5}, {lon:.5}"))
}

fn gps_decimal(exif: &exif::Exif, tag: Tag, ref_tag: Tag, negative_ref: char) -> Option<f64> {
    let field = exif.get_field(tag, In::PRIMARY)?;
    let exif::Value::Rational(parts) = &field.value else {
        return None;
    };
    if parts.len() < 3 {
        return None;
    }
    let degrees = parts[0].to_f64();
    let minutes = parts[1].to_f64();
    let seconds = parts[2].to_f64();
    let mut decimal = degrees + minutes / 60.0 + seconds / 3600.0;
    let reference = exif
        .get_field(ref_tag, In::PRIMARY)
        .map(|f| f.display_value().to_string())
        .unwrap_or_default();
    if reference.starts_with(negative_ref) {
        decimal = -decimal;
    }
    Some(decimal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use exif::experimental::Writer;
    use exif::{Field, Rational, Value};
    use image::{ExtendedColorType, ImageEncoder};

    fn field(tag: Tag, value: Value) -> Field {
        Field {
            tag,
            ifd_num: In::PRIMARY,
            value,
        }
    }

    fn ascii(tag: Tag, text: &str) -> Field {
        field(tag, Value::Ascii(vec![text.as_bytes().to_vec()]))
    }

    fn short(tag: Tag, number: u16) -> Field {
        field(tag, Value::Short(vec![number]))
    }

    fn rational(tag: Tag, parts: &[(u32, u32)]) -> Field {
        let parts = parts
            .iter()
            .map(|&(num, denom)| Rational { num, denom })
            .collect();
        field(tag, Value::Rational(parts))
    }

    /// The little-endian TIFF block of an EXIF segment.
    fn exif_block(fields: &[Field]) -> Vec<u8> {
        let mut writer = Writer::new();
        for field in fields {
            writer.push_field(field);
        }
        let mut out = std::io::Cursor::new(Vec::new());
        writer.write(&mut out, true).unwrap();
        out.into_inner()
    }

    /// A black 16x8 JPEG with `fields` as EXIF.
    fn jpeg_with_exif(path: &Path, fields: &[Field]) {
        let file = File::create(path).unwrap();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(file, 90);
        encoder.set_exif_metadata(exif_block(fields)).unwrap();
        encoder
            .write_image(&[0; 16 * 8 * 3], 16, 8, ExtendedColorType::Rgb8)
            .unwrap();
    }

    fn exif_rows_for(fields: &[Field]) -> Vec<(String, String)> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("photo.jpg");
        jpeg_with_exif(&path, fields);
        exif_rows(&path)
    }

    fn row<'a>(rows: &'a [(String, String)], label: &str) -> Option<&'a str> {
        rows.iter()
            .find(|(candidate, _)| candidate == label)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn human_size_scales_units() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2.0 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(human_size(3 * 1024 * 1024 * 1024), "3.0 GB");
        assert_eq!(human_size(2048 * 1024 * 1024 * 1024 * 1024), "2048.0 TB");
    }

    #[test]
    fn megapixels_drop_decimal_from_ten() {
        assert_eq!(megapixels(1920, 1080), "2.1 MP");
        assert_eq!(megapixels(4000, 3000), "12 MP");
        assert_eq!(megapixels(16, 8), "0.0 MP");
    }

    #[test]
    fn missing_file_has_name_folder_format() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing").join("gone.jpeg");
        let info = load(&path);
        let labels: Vec<&str> = info.rows.iter().map(|(label, _)| label.as_str()).collect();
        assert_eq!(labels, ["", "Name", "Folder", "Format"]);
        assert_eq!(row(&info.rows, ""), Some("File"));
        assert_eq!(row(&info.rows, "Name"), Some("gone.jpeg"));
        assert_eq!(row(&info.rows, "Format"), Some("JPEG"));
    }

    #[test]
    fn jpeg_has_file_and_exif_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("photo.jpg");
        jpeg_with_exif(
            &path,
            &[
                ascii(Tag::Make, "Canon"),
                ascii(Tag::Model, "Canon EOS R5"),
                ascii(Tag::LensModel, "RF 50mm"),
                ascii(Tag::DateTimeOriginal, "2024:05:01 10:20:30"),
                rational(Tag::ExposureTime, &[(1, 250)]),
                rational(Tag::FNumber, &[(28, 10)]),
                short(Tag::PhotographicSensitivity, 400),
                rational(Tag::FocalLength, &[(50, 1)]),
                short(Tag::FocalLengthIn35mmFilm, 75),
                short(Tag::Flash, 0),
                short(Tag::Orientation, 1),
                rational(Tag::GPSLatitude, &[(52, 1), (30, 1), (0, 1)]),
                ascii(Tag::GPSLatitudeRef, "N"),
                rational(Tag::GPSLongitude, &[(13, 1), (24, 1), (0, 1)]),
                ascii(Tag::GPSLongitudeRef, "W"),
                ascii(Tag::Software, "phewr test"),
                ascii(Tag::ImageDescription, "A test photo"),
            ],
        );
        let info = load(&path);
        let size = std::fs::metadata(&path).unwrap().len();
        let rows = &info.rows;

        assert_eq!(row(rows, "Name"), Some("photo.jpg"));
        assert_eq!(row(rows, "Size"), Some(human_size(size).as_str()));
        assert!(row(rows, "Modified").is_some());
        assert_eq!(row(rows, "Dimensions"), Some("16 x 8 (0.0 MP)"));
        assert_eq!(row(rows, "Format"), Some("JPG"));
        assert!(rows.contains(&(String::new(), "EXIF".to_owned())));
        assert_eq!(row(rows, "Camera"), Some("Canon EOS R5"));
        assert_eq!(row(rows, "Lens"), Some("RF 50mm"));
        assert_eq!(row(rows, "Taken"), Some("2024-05-01 10:20:30"));
        assert_eq!(row(rows, "ISO"), Some("400"));
        assert_eq!(row(rows, "GPS"), Some("52.50000, -13.40000"));
        assert_eq!(row(rows, "Software"), Some("phewr test"));
        assert_eq!(row(rows, "Description"), Some("A test photo"));
        let focal = row(rows, "Focal length").unwrap();
        assert!(focal.ends_with("eq.)"), "focal: {focal}");
        for label in ["Exposure", "Aperture", "Flash", "Orientation"] {
            assert!(row(rows, label).is_some(), "missing {label}");
        }
    }

    #[test]
    fn camera_joins_make_and_model() {
        let both = exif_rows_for(&[ascii(Tag::Make, "Nikon"), ascii(Tag::Model, "Z 6")]);
        assert_eq!(row(&both, "Camera"), Some("Nikon Z 6"));

        let make = exif_rows_for(&[ascii(Tag::Make, "Nikon")]);
        assert_eq!(row(&make, "Camera"), Some("Nikon"));

        let model = exif_rows_for(&[ascii(Tag::Model, "Z 6")]);
        assert_eq!(row(&model, "Camera"), Some("Z 6"));
    }

    #[test]
    fn focal_length_shows_existing_values() {
        let focal = exif_rows_for(&[rational(Tag::FocalLength, &[(35, 1)])]);
        let value = row(&focal, "Focal length").unwrap();
        assert!(!value.contains("eq."), "focal: {value}");

        let equivalent = exif_rows_for(&[short(Tag::FocalLengthIn35mmFilm, 28)]);
        assert_eq!(row(&equivalent, "Focal length"), Some("28 mm eq."));
    }

    #[test]
    fn exif_rows_skip_empty_fields() {
        let rows = exif_rows_for(&[
            ascii(Tag::DateTime, "2023:01:02 03:04:05"),
            ascii(Tag::ImageDescription, " "),
            rational(Tag::GPSLatitude, &[(52, 1), (30, 1)]),
            ascii(Tag::GPSLatitudeRef, "S"),
            rational(Tag::GPSLongitude, &[(13, 1), (24, 1), (0, 1)]),
        ]);
        assert_eq!(row(&rows, "Taken"), Some("2023-01-02 03:04:05"));
        assert_eq!(row(&rows, "Description"), None);
        assert_eq!(row(&rows, "GPS"), None);
        assert_eq!(row(&rows, "Camera"), None);
    }

    #[test]
    fn gps_reads_only_rational_coordinates() {
        let rows = exif_rows_for(&[
            ascii(Tag::GPSLatitude, "52"),
            rational(Tag::GPSLongitude, &[(13, 1), (24, 1), (0, 1)]),
            ascii(Tag::Software, "only software"),
        ]);
        assert_eq!(row(&rows, "GPS"), None);
        assert_eq!(row(&rows, "Software"), Some("only software"));

        let south_east = exif_rows_for(&[
            rational(Tag::GPSLatitude, &[(33, 1), (51, 1), (36, 1)]),
            ascii(Tag::GPSLatitudeRef, "S"),
            rational(Tag::GPSLongitude, &[(151, 1), (12, 1), (36, 1)]),
            ascii(Tag::GPSLongitudeRef, "E"),
        ]);
        assert_eq!(row(&south_east, "GPS"), Some("-33.86000, 151.21000"));
    }

    #[test]
    fn no_exif_gives_no_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plain.jpg");
        image::DynamicImage::new_rgb8(4, 4).save(&path).unwrap();
        assert!(exif_rows(&path).is_empty());
        assert!(exif_rows(&dir.path().join("missing.jpg")).is_empty());
    }

    #[cfg(any(feature = "turbo", all(feature = "sips", target_os = "macos")))]
    #[test]
    fn orientation_accepts_one_to_eight() {
        let jpeg = |orientation: Option<u16>| {
            let mut bytes = Vec::new();
            let mut encoder = image::codecs::jpeg::JpegEncoder::new(&mut bytes);
            if let Some(orientation) = orientation {
                encoder
                    .set_exif_metadata(exif_block(&[short(Tag::Orientation, orientation)]))
                    .unwrap();
            }
            encoder
                .write_image(&[0; 4 * 4 * 3], 4, 4, ExtendedColorType::Rgb8)
                .unwrap();
            bytes
        };
        assert_eq!(orientation_from_bytes(&jpeg(Some(6))), Some(6));
        assert_eq!(orientation_from_bytes(&jpeg(Some(9))), None);
        assert_eq!(orientation_from_bytes(&jpeg(None)), None);
        assert_eq!(orientation_from_bytes(b"not a jpeg"), None);
    }

    #[test]
    fn png_without_exif_has_file_rows() {
        let file = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        image::DynamicImage::new_rgba8(12, 7)
            .save(file.path())
            .unwrap();
        let info = load(file.path());

        let dims = info
            .rows
            .iter()
            .find(|(label, _)| label == "Dimensions")
            .map(|(_, value)| value.clone())
            .unwrap();
        assert!(dims.starts_with("12 x 7"), "dims: {dims}");
        assert!(!info.rows.iter().any(|(_, value)| value == "EXIF"));
    }

    #[test]
    fn shows_tga_and_exr_dimensions() {
        let dir = tempfile::tempdir().unwrap();
        let tga = dir.path().join("a.tga");
        image::DynamicImage::new_rgb8(5, 3).save(&tga).unwrap();
        let exr = dir.path().join("a.exr");
        image::DynamicImage::new_rgb32f(4, 2).save(&exr).unwrap();
        let dims = |path: &Path| row(&load(path).rows, "Dimensions").map(str::to_owned);
        assert!(dims(&tga).unwrap().starts_with("5 x 3"));
        assert!(dims(&exr).unwrap().starts_with("4 x 2"));
    }

    #[cfg(all(feature = "sips", target_os = "macos"))]
    #[test]
    fn shows_psd_dimensions() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("a.png");
        image::DynamicImage::new_rgb8(6, 4).save(&png).unwrap();
        let psd = dir.path().join("a.psd");
        let status = std::process::Command::new("/usr/bin/sips")
            .args(["-s", "format", "psd"])
            .arg(&png)
            .arg("--out")
            .arg(&psd)
            .output()
            .unwrap()
            .status;
        assert!(status.success());
        let info = load(&psd);
        let dims = row(&info.rows, "Dimensions").unwrap();
        assert!(dims.starts_with("6 x 4"), "dims: {dims}");
    }
}
