//! The name of the app in the files, the threads and the variables of the module. Each copy takes
//! the package name from the `Cargo.toml` of its app, so two apps never share a cache folder, a
//! cache key or a temp file, and the module holds no app name of its own.

/// The package name of the app: the cache folder, the thread names and the temp files.
pub(crate) const APP_NAME: &str = env!("CARGO_PKG_NAME");

/// The environment variable of the real-file test `name`: `<APP>_TEST_JPEG` for `JPEG`, with the
/// package name in upper case.
#[cfg(test)]
pub(crate) fn test_variable(name: &str) -> String {
    format!("{}_TEST_{name}", APP_NAME.to_ascii_uppercase())
}

/// The file of the real-file test `name`. Without the variable the test skips with a note.
#[cfg(test)]
pub(crate) fn test_file(name: &str) -> Option<std::path::PathBuf> {
    crate::pictures::decode::real_test_file(&test_variable(name))
}

#[cfg(test)]
mod tests {
    use super::{APP_NAME, test_file, test_variable};

    #[test]
    fn real_file_variables_carry_the_app_name() {
        let prefix = APP_NAME.to_ascii_uppercase();
        assert_eq!(test_variable("JPEG"), format!("{prefix}_TEST_JPEG"));
        assert_eq!(test_file("UNSET_IN_ANY_RUN"), None);
    }
}
