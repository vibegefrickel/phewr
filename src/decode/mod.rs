//! The decoders outside the `image` crate and libheif, and the helpers of the decode routes:
//! JPEG XL, macOS `sips`, the tone map of float images and the HEIF box reader.

#[cfg(feature = "heic")]
pub(crate) mod heif_boxes;
pub(crate) mod jxl;
#[cfg(all(feature = "sips", target_os = "macos"))]
pub(crate) mod sips;
pub(crate) mod tonemap;

use crate::scanner;
use std::path::Path;

/// The decoder of a file outside libheif. libheif has its own check (`decodes_with_libheif`),
/// because the info panel uses it too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Route {
    /// The `image` crate, with libjpeg-turbo for JPEG. Unknown extensions go here too, so that
    /// the content decides.
    Raster,
    Jxl,
    #[cfg(all(feature = "sips", target_os = "macos"))]
    Sips,
}

impl Route {
    /// Pick the decoder from the extension. The check compares without an allocation and tests
    /// the `image` formats first, so the JPEG and PNG paths pay almost nothing.
    pub(crate) fn for_path(path: &Path) -> Self {
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            return Self::Raster;
        };
        let listed = |list: &[&str]| list.iter().any(|e| ext.eq_ignore_ascii_case(e));
        if listed(scanner::IMAGE_EXTENSIONS) {
            return Self::Raster;
        }
        if listed(scanner::JXL_EXTENSIONS) {
            return Self::Jxl;
        }
        #[cfg(all(feature = "sips", target_os = "macos"))]
        if listed(scanner::SIPS_EXTENSIONS) {
            return Self::Sips;
        }
        Self::Raster
    }
}

/// The real file that the ignored test reads from `variable`. Without the variable the test
/// skips with a note, so that `just test-real` runs with only some of the variables set.
#[cfg(test)]
pub(crate) fn real_test_file(variable: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os(variable).map(std::path::PathBuf::from);
    if path.is_none() {
        // clippy.toml allows output in tests. `just test-real` shows it.
        eprintln!("skipped: {variable} is not set");
    }
    path
}

#[cfg(test)]
mod tests {
    use super::Route;
    use std::path::Path;

    #[test]
    fn the_extension_picks_the_route() {
        let route = |name: &str| Route::for_path(Path::new(name));
        assert_eq!(route("a.JPG"), Route::Raster);
        assert_eq!(route("a.exr"), Route::Raster);
        assert_eq!(route("a.JXL"), Route::Jxl);
        assert_eq!(route("unknown.xyz"), Route::Raster);
        assert_eq!(route("no-extension"), Route::Raster);
        #[cfg(all(feature = "sips", target_os = "macos"))]
        assert_eq!(route("a.CR2"), Route::Sips);
    }
}
