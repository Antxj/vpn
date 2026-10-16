//! Linux: where the app keeps its data ($XDG_CONFIG_HOME/vpn, usually ~/.config/vpn).

use std::path::PathBuf;

pub use crate::platform::unix::paths::pasta_home;

/// App data folder (settings and logs): $XDG_CONFIG_HOME/vpn, usually ~/.config/vpn.
pub fn app_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| pasta_home().join(".config"))
        .join("vpn")
}

/// Warning about where the app runs from (only macOS has one: a temporary
/// copy of a downloaded app).
pub fn aviso_de_local() -> Option<String> {
    None
}
