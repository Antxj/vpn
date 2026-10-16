//! macOS: where the app keeps its data (~/Library/Application Support/VPN).

use std::path::PathBuf;

pub use crate::platform::unix::paths::pasta_home;

/// App data folder (settings and logs).
pub fn app_dir() -> PathBuf {
    pasta_home()
        .join("Library")
        .join("Application Support")
        .join("VPN")
}

/// Warning when the app runs from where it cannot stay: macOS runs an app
/// downloaded from the internet that was not moved to Applications from a
/// temporary read-only copy (App Translocation), so updates and "start with
/// the system" would not last.
pub fn aviso_de_local() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    exe.to_string_lossy()
        .contains("/AppTranslocation/")
        .then(|| {
            tr!(
                "O VPN está rodando de uma cópia temporária do macOS.\nMova o \
             VPN.app para Aplicativos e abra de lá.",
                "VPN is running from a temporary copy made by macOS.\nMove VPN.app \
             to Applications and open it from there."
            )
            .to_string()
        })
}
