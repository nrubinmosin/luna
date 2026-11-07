//! An account folder of a test's own.

use super::*;

pub(super) fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("luna-acct-settings-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}
