//! Windows: where OpenVPN is installed and where its configs usually are.

use std::path::PathBuf;

/// Where to download OpenVPN when the embedded installer is not available.
pub const PAGINA_DE_DOWNLOAD: &str = "https://openvpn.net/community-downloads/";

const OPENVPN_CANDIDATES: &[&str] = &[
    r"C:\Program Files\OpenVPN\bin\openvpn.exe",
    r"C:\Program Files (x86)\OpenVPN\bin\openvpn.exe",
];
/// Key written by the OpenVPN installer (finds installations outside the
/// default path, e.g. on another drive).
const OPENVPN_REG_KEY: &str = r"SOFTWARE\OpenVPN";

/// Reads HKLM\SOFTWARE\OpenVPN\exe_path (written by the official installer).
fn openvpn_from_registry() -> Option<PathBuf> {
    use windows_sys::Win32::System::Registry::{
        RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6432KEY,
        RRF_SUBKEY_WOW6464KEY,
    };

    let key = super::single::wide(OPENVPN_REG_KEY);
    let value = super::single::wide("exe_path");
    // tries the 64-bit and the 32-bit registry views
    for view in [RRF_SUBKEY_WOW6464KEY, RRF_SUBKEY_WOW6432KEY] {
        let mut buf = [0u16; 512];
        let mut len = (buf.len() * 2) as u32;
        let ok = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ | view,
                std::ptr::null_mut(),
                buf.as_mut_ptr() as *mut std::ffi::c_void,
                &mut len,
            )
        };
        if ok == 0 {
            let chars = (len as usize / 2).saturating_sub(1).min(buf.len());
            let path = PathBuf::from(String::from_utf16_lossy(&buf[..chars]));
            if path.exists() {
                return Some(path);
            }
        }
    }
    None
}

/// openvpn.exe of the installation (default folders or the path in the registry).
pub fn encontrar() -> Option<PathBuf> {
    OPENVPN_CANDIDATES
        .iter()
        .map(PathBuf::from)
        .find(|p| p.exists())
        .or_else(openvpn_from_registry)
}

/// Where .ovpn files usually are (first folder of the "choose file" dialog).
pub fn pastas_de_config() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(profile) = std::env::var("USERPROFILE") {
        dirs.push(PathBuf::from(profile).join("OpenVPN").join("config"));
    }
    dirs.push(PathBuf::from(r"C:\Program Files\OpenVPN\config"));
    dirs
}
