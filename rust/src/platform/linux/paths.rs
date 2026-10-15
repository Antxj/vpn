//! Linux: where the app keeps its data ($XDG_CONFIG_HOME/vpn, usually ~/.config/vpn).

use std::path::PathBuf;

/// The user's home folder.
pub fn pasta_home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

/// App data folder (settings and logs): $XDG_CONFIG_HOME/vpn, usually ~/.config/vpn.
pub fn app_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| pasta_home().join(".config"))
        .join("vpn")
}
