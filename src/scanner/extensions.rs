//! The supported extensions and the extension test of the scanner and the decoders.

use std::path::Path;

/// JPEG and PNG come first, because most files are JPEG or PNG. The decode reads an `.apng`
/// file as PNG.
pub(crate) const IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "apng", "gif", "webp", "bmp", "tiff", "tif", "ico", "exr", "hdr", "qoi",
    "tga",
];

pub(crate) const JXL_EXTENSIONS: &[&str] = &["jxl"];

/// The formats that `sips` reads and the `image` crate does not. `sips --formats` lists them.
#[cfg(all(feature = "sips", target_os = "macos"))]
pub(crate) const SIPS_EXTENSIONS: &[&str] = &[
    // Camera RAW.
    "3fr", "arw", "cr2", "cr3", "crw", "dcr", "dng", "erf", "fff", "iiq", "mos", "mrw", "nef",
    "nrw", "orf", "pef", "raf", "raw", "rw2", "rwl", "sr2", "srf", "srw",
    // Other formats of ImageIO.
    "psd", "jp2", "pdf", "svg", "icns", "sgi", "dds", "pict",
];

/// The `image` crate has no AVIF decoder.
#[cfg(feature = "heic")]
pub(crate) const LIBHEIF_EXTENSIONS: &[&str] = &["heic", "heif", "hif", "avif"];

#[cfg(feature = "video")]
pub(crate) const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mov", "mkv", "avi", "webm", "m4v"];

/// It ignores ASCII case and allocates nothing.
pub(crate) fn has_extension(path: &Path, list: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| list.iter().any(|e| ext.eq_ignore_ascii_case(e)))
}

pub(crate) fn is_supported(path: &Path) -> bool {
    if has_extension(path, IMAGE_EXTENSIONS) || has_extension(path, JXL_EXTENSIONS) {
        return true;
    }
    #[cfg(all(feature = "sips", target_os = "macos"))]
    if has_extension(path, SIPS_EXTENSIONS) {
        return true;
    }
    #[cfg(feature = "heic")]
    if has_extension(path, LIBHEIF_EXTENSIONS) {
        return true;
    }
    #[cfg(feature = "video")]
    if has_extension(path, VIDEO_EXTENSIONS) {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::is_supported;
    use std::path::Path;

    #[test]
    fn ignores_extension_case() {
        assert!(is_supported(Path::new("synthetic.PNG")));
        assert!(is_supported(Path::new("synthetic.jpeg")));
    }

    #[cfg(feature = "heic")]
    #[test]
    fn supports_heic() {
        assert!(is_supported(Path::new("IMG_0001.HEIC")));
        assert!(is_supported(Path::new("photo.heif")));
        assert!(is_supported(Path::new("photo.AVIF")));
    }

    #[cfg(not(feature = "heic"))]
    #[test]
    fn avif_needs_heic_feature() {
        assert!(!is_supported(Path::new("photo.avif")));
    }

    #[cfg(feature = "video")]
    #[test]
    fn supports_video() {
        assert!(is_supported(Path::new("clip.MP4")));
        assert!(is_supported(Path::new("clip.webm")));
    }

    #[test]
    fn supports_image_and_jxl() {
        for name in ["a.exr", "a.HDR", "a.qoi", "a.tga", "a.jxl", "a.APNG"] {
            assert!(is_supported(Path::new(name)), "{name}");
        }
    }

    #[cfg(all(feature = "sips", target_os = "macos"))]
    #[test]
    fn supports_sips_formats() {
        for name in ["a.CR3", "a.nef", "a.dng", "a.psd", "a.pdf", "a.svg"] {
            assert!(is_supported(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn rejects_unknown_extensions() {
        assert!(!is_supported(Path::new("synthetic.txt")));
        assert!(!is_supported(Path::new("synthetic")));
    }
}
