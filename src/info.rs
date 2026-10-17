//! File and EXIF metadata for the info panel.

use chrono::{DateTime, Local};
use exif::{In, Tag};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

/// Rows shown in the info panel. An empty label marks a section heading.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ImageInfo {
    pub rows: Vec<(String, String)>,
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

fn megapixels(w: u32, h: u32) -> String {
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

fn dimensions(path: &Path) -> Option<(u32, u32)> {
    #[cfg(feature = "heic")]
    if crate::app::decodes_with_libheif(path) {
        return crate::app::libheif_dimensions(path);
    }
    let file = File::open(path).ok()?;
    image::ImageReader::new(BufReader::new(file))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
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

fn read_exif(path: &Path) -> Option<exif::Exif> {
    let file = File::open(path).ok()?;
    exif::Reader::new()
        .read_from_container(&mut BufReader::new(file))
        .ok()
}

/// Read the EXIF orientation tag (1..=8) from JPEG bytes in memory, because libjpeg-turbo
/// ignores it and the file is already read.
#[cfg(feature = "turbo")]
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

    #[test]
    fn human_size_scales_units() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2.0 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
    }

    #[test]
    fn info_for_a_png_without_exif_has_file_rows_only() {
        let path =
            std::env::temp_dir().join(format!("viewr-synthetic-info-{}.png", std::process::id()));
        image::DynamicImage::new_rgba8(12, 7).save(&path).unwrap();
        let info = load(&path);
        std::fs::remove_file(&path).unwrap();

        let dims = info
            .rows
            .iter()
            .find(|(label, _)| label == "Dimensions")
            .map(|(_, value)| value.clone())
            .unwrap();
        assert!(dims.starts_with("12 x 7"), "dims: {dims}");
        assert!(!info.rows.iter().any(|(_, value)| value == "EXIF"));
    }
}
