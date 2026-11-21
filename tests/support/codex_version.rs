use std::{
    fs,
    path::{Path, PathBuf},
};

/// Per-test metadata without environment changes or writing an executable during
/// parallel process spawning (which can inherit a writer and cause ETXTBSY).
pub fn wrapper(root: &Path, delegate: &Path, version: &str) -> PathBuf {
    let binary = root.join("codex-wrapper.py");
    std::os::unix::fs::symlink(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/app-server/version_wrapper.py"),
        &binary,
    )
    .unwrap();
    fs::write(
        binary.with_extension("json"),
        serde_json::json!({"delegate":delegate}).to_string(),
    )
    .unwrap();
    set(&binary, version);
    binary
}

pub fn set(binary: &Path, version: &str) {
    fs::write(binary.with_extension("version"), version).unwrap();
}
