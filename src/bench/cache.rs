//! The file format of the thumbnail disk cache.

use super::{report, scanned_app, test_dir};
use std::time::Instant;

struct CacheFormat {
    name: &'static str,
    encode: fn(&image::RgbaImage) -> Vec<u8>,
    decode: fn(&[u8]) -> image::RgbaImage,
}

fn encode_with(image: &image::RgbaImage, format: image::ImageFormat) -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, format).unwrap();
    bytes.into_inner()
}

fn decode_with(bytes: &[u8], format: image::ImageFormat) -> image::RgbaImage {
    image::load_from_memory_with_format(bytes, format)
        .unwrap()
        .into_rgba8()
}

fn raw_header(image: &image::RgbaImage) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(8 + image.as_raw().len());
    bytes.extend_from_slice(&image.width().to_le_bytes());
    bytes.extend_from_slice(&image.height().to_le_bytes());
    bytes
}

fn raw_dimensions(bytes: &[u8]) -> (u32, u32) {
    (
        u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
        u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
    )
}

/// Raw RGBA is the lower bound of the read.
const CACHE_FORMATS: [CacheFormat; 3] = [
    CacheFormat {
        name: "png",
        encode: |image| encode_with(image, image::ImageFormat::Png),
        decode: |bytes| decode_with(bytes, image::ImageFormat::Png),
    },
    CacheFormat {
        name: "qoi",
        encode: |image| encode_with(image, image::ImageFormat::Qoi),
        decode: |bytes| decode_with(bytes, image::ImageFormat::Qoi),
    },
    CacheFormat {
        name: "raw rgba",
        encode: |image| {
            let mut bytes = raw_header(image);
            bytes.extend_from_slice(image.as_raw());
            bytes
        },
        decode: |bytes| {
            let (width, height) = raw_dimensions(bytes);
            image::RgbaImage::from_raw(width, height, bytes[8..].to_vec()).unwrap()
        },
    },
];

/// The disk cache formats on the first screen: cold encode and write, warm read and decode, bytes.
#[test]
#[ignore = "reads the folder PHEWR_TEST_DIR"]
fn formats() {
    let Some(dir) = test_dir() else {
        report("skipped: PHEWR_TEST_DIR is not set");
        return;
    };
    let bench = scanned_app(&dir);
    let app = &bench.app;
    let target =
        crate::pictures::resize::padding::thumb_target_px(app.gallery.thumb_box(), app.cell_px);
    let thumbnails: Vec<image::RgbaImage> = app
        .gallery
        .visible_items()
        .filter_map(|(_, index)| {
            crate::pictures::resize::load_and_resize_px(
                &app.images[index],
                target,
                crate::pictures::decode::EmbeddedThumbnail::Allow,
            )
            .ok()
        })
        .collect();
    let pixel_bytes: usize = thumbnails.iter().map(|image| image.as_raw().len()).sum();
    report(&format!(
        "cache formats: {} thumbnails for a box of {target:?}, {} KB of pixels each on average",
        thumbnails.len(),
        pixel_bytes / thumbnails.len().max(1) / 1024
    ));
    for format in &CACHE_FORMATS {
        let files = tempfile::tempdir().unwrap();
        let start = Instant::now();
        let encoded: Vec<Vec<u8>> = thumbnails.iter().map(format.encode).collect();
        let encode = start.elapsed();
        for (index, bytes) in encoded.iter().enumerate() {
            std::fs::write(files.path().join(index.to_string()), bytes).unwrap();
        }
        let write = start.elapsed();
        let start = Instant::now();
        for (index, image) in thumbnails.iter().enumerate() {
            let bytes = std::fs::read(files.path().join(index.to_string())).unwrap();
            assert_eq!((format.decode)(&bytes).as_raw(), image.as_raw());
        }
        let read = start.elapsed();
        let size: usize = encoded.iter().map(Vec::len).sum();
        let count = thumbnails.len().max(1) as f64;
        report(&format!(
            "cache format {}: encode {:.2} ms, with the write {:.2} ms, read and decode {:.2} ms per thumbnail, {} KB per file ({:.0} % of the pixels)",
            format.name,
            encode.as_secs_f64() * 1000.0 / count,
            write.as_secs_f64() * 1000.0 / count,
            read.as_secs_f64() * 1000.0 / count,
            size / thumbnails.len().max(1) / 1024,
            size as f64 * 100.0 / pixel_bytes.max(1) as f64
        ));
    }
}
