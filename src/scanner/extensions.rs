//! The supported extensions of the scanner. The extensions of the decoders and the extension
//! test live in `tuilibr::pictures::decode::extensions`.

use std::path::Path;
#[cfg(feature = "jxl")]
use tuilibr::pictures::decode::extensions::JXL_EXTENSIONS;
#[cfg(feature = "heic")]
use tuilibr::pictures::decode::extensions::LIBHEIF_EXTENSIONS;
#[cfg(all(feature = "sips", target_os = "macos"))]
use tuilibr::pictures::decode::extensions::SIPS_EXTENSIONS;
use tuilibr::pictures::decode::extensions::{IMAGE_EXTENSIONS, has_extension};

#[cfg(feature = "video")]
pub(crate) const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mov", "mkv", "avi", "webm", "m4v"];

pub(crate) fn is_supported(path: &Path) -> bool {
    let lists: &[&[&str]] = &[
        IMAGE_EXTENSIONS,
        #[cfg(feature = "jxl")]
        JXL_EXTENSIONS,
        #[cfg(all(feature = "sips", target_os = "macos"))]
        SIPS_EXTENSIONS,
        #[cfg(feature = "heic")]
        LIBHEIF_EXTENSIONS,
        #[cfg(feature = "video")]
        VIDEO_EXTENSIONS,
    ];
    lists.iter().any(|list| has_extension(path, list))
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
    fn supports_image_formats() {
        for name in ["a.exr", "a.HDR", "a.qoi", "a.tga", "a.APNG"] {
            assert!(is_supported(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn jxl_needs_jxl_feature() {
        assert_eq!(is_supported(Path::new("a.jxl")), cfg!(feature = "jxl"));
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
