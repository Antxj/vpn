//! Linux: where OpenVPN is installed (by the distribution's package manager)
//! and where its configs usually are.

use std::path::PathBuf;

/// How to install OpenVPN with the distribution's package manager.
pub const PAGINA_DE_DOWNLOAD: &str = "https://github.com/Antxj/vpn#linux";

const OPENVPN_CANDIDATES: &[&str] = &["/usr/sbin/openvpn", "/usr/bin/openvpn", "/usr/local/sbin/openvpn"];

pub fn encontrar() -> Option<PathBuf> {
    OPENVPN_CANDIDATES.iter().map(PathBuf::from).find(|p| p.exists())
}

/// Where .ovpn files usually are (first folder of the "choose file" dialog).
pub fn pastas_de_config() -> Vec<PathBuf> {
    vec![super::paths::pasta_home(), PathBuf::from("/etc/openvpn/client")]
}
