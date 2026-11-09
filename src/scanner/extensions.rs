//! The supported extensions of the scanner. The extensions of the decoders and the extension
//! test live in `crate::pictures::decode::extensions`.

#[cfg(feature = "heic")]
use crate::pictures::decode::extensions::LIBHEIF_EXTENSIONS;
#[cfg(all(feature = "sips", target_os = "macos"))]
use crate::pictures::decode::extensions::SIPS_EXTENSIONS;
use crate::pictures::decode::extensions::{IMAGE_EXTENSIONS, JXL_EXTENSIONS, has_extension};
use std::path::Path;

#[cfg(feature = "video")]
pub(crate) const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mov", "mkv", "avi", "webm", "m4v"];

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
