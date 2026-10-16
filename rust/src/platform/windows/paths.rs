//! Windows: where the app keeps its data (%APPDATA%\VPN).

use std::path::PathBuf;

fn appdata() -> PathBuf {
    PathBuf::from(std::env::var("APPDATA").unwrap_or_else(|_| ".".into()))
}

/// App data folder (settings and logs).
pub fn app_dir() -> PathBuf {
    appdata().join("VPN")
}

/// Warning about where the app runs from (only macOS has one: a temporary
/// copy of a downloaded app).
pub fn aviso_de_local() -> Option<String> {
    None
}
