//! Linux side of the updater: the asset is the executable itself (the
//! download and the checks are in unix/updater.rs).

use std::path::Path;

pub use crate::platform::unix::updater::{
    aguardar_processo, assinante, http_get, sha256_hex, tornar_executavel,
};

/// Name of the release asset that contains the app.
pub const ARQUIVO: &str = "VPN-linux-x86_64";
/// The downloaded file must start like an executable (ELF).
pub const CABECALHO: &[u8] = b"\x7fELF";

/// The downloaded file is the executable itself.
pub fn extrair(dados: Vec<u8>) -> Result<Vec<u8>, String> {
    Ok(dados)
}

/// Nothing besides the executable to update.
pub fn depois_de_trocar(_exe: &Path) {}
