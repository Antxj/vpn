//! Linux: where OpenVPN is installed (by the distribution's package manager)
//! and where its configs usually are.

use std::path::PathBuf;

/// How to install OpenVPN with the distribution's package manager.
pub const PAGINA_DE_DOWNLOAD: &str = "https://github.com/Antxj/vpn#linux";

const OPENVPN_CANDIDATES: &[&str] = &[
    "/usr/sbin/openvpn",
    "/usr/bin/openvpn",
    "/usr/local/sbin/openvpn",
];

pub fn encontrar() -> Option<PathBuf> {
    OPENVPN_CANDIDATES
        .iter()
        .map(PathBuf::from)
        .find(|p| p.exists())
}

/// Where .ovpn files usually are (first folder of the "choose file" dialog).
pub fn pastas_de_config() -> Vec<PathBuf> {
    vec![
        super::paths::pasta_home(),
        PathBuf::from("/etc/openvpn/client"),
    ]
}

/// Older distributions (Ubuntu 22.04, Linux Mint 21) still ship OpenVPN 2.5,
/// no longer maintained by the OpenVPN project: the app recommends updating.
pub const RECOMMENDED_MINIMUM: Option<crate::openvpn_version::Version> =
    Some(crate::openvpn_version::Version::new(2, 6, 0));

/// How to install an up-to-date OpenVPN from the OpenVPN project's repository.
pub const UPDATE_HELP_URL: &str = "https://github.com/Antxj/vpn#updating-openvpn-on-linux";

/// The text of `openvpn --version`.
pub fn version_output(path: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new(path)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}
