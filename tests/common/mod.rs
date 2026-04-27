use std::path::PathBuf;

/// Path to the bundled sample TOML config used by integration tests.
pub fn sample_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("samples/sample.toml")
}
