//! macOS: the app brings its own OpenVPN (built from the official source by
//! build-release-macos.sh and embedded in the executable). The one-time
//! authorization installs it root-owned (service.rs): Homebrew's openvpn
//! belongs to the user, and running it as root without a password would let
//! any program of the user become root.

use std::path::PathBuf;

/// The macOS section of the README (this build has no OpenVPN inside).
pub const PAGINA_DE_DOWNLOAD: &str = "https://github.com/Antxj/vpn#macos";

/// openvpn for this processor (empty in builds made without it).
pub static EMBUTIDO: &[u8] = include_bytes!(env!("VPN_OPENVPN_MACOS"));
/// OpenVPN's DNS script for macOS (distro/dns-scripts/macos-dns-updown.sh).
pub static SCRIPT_DE_DNS: &[u8] = include_bytes!(env!("VPN_DNS_MACOS"));

/// The app can connect (OpenVPN is inside it). The path is where the
/// authorization installs it.
pub fn encontrar() -> Option<PathBuf> {
    (!EMBUTIDO.is_empty()).then(|| PathBuf::from(super::service::OPENVPN))
}

/// Where .ovpn files usually are (first folder of the "choose file" dialog).
pub fn pastas_de_config() -> Vec<PathBuf> {
    let home = super::paths::pasta_home();
    vec![home.join("Downloads"), home]
}
