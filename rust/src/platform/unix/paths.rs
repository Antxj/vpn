//! Linux and macOS: the user's home folder.

use std::path::PathBuf;

/// The user's home folder.
pub fn pasta_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}
