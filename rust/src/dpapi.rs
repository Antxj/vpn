//! DPAPI (CryptProtectData) + persisted settings.
//!
//! %APPDATA%/VPN/contas.dat: a JSON encrypted with DPAPI in the user scope,
//! with the preferences and the list of accounts. On Linux the file lives in
//! ~/.config/vpn and is encrypted with a key from the desktop keyring
//! (see linux/segredo.rs).

use crate::accounts::Conta;
use serde::{Deserialize, Serialize};
#[cfg(windows)]
use std::ffi::c_void;
use std::path::PathBuf;
#[cfg(windows)]
use std::ptr;

#[cfg(windows)]
#[repr(C)]
struct DataBlob {
    cb_data: u32,
    pb_data: *mut u8,
}

#[cfg(windows)]
#[link(name = "crypt32")]
extern "system" {
    fn CryptProtectData(
        data_in: *const DataBlob,
        descr: *const u16,
        entropy: *const DataBlob,
        reserved: *mut c_void,
        prompt: *mut c_void,
        flags: u32,
        data_out: *mut DataBlob,
    ) -> i32;
    fn CryptUnprotectData(
        data_in: *const DataBlob,
        descr: *mut *mut u16,
        entropy: *const DataBlob,
        reserved: *mut c_void,
        prompt: *mut c_void,
        flags: u32,
        data_out: *mut DataBlob,
    ) -> i32;
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn LocalFree(mem: *mut c_void) -> *mut c_void;
}

#[cfg(windows)]
fn dpapi(data: &[u8], protect: bool) -> Option<Vec<u8>> {
    let blob_in = DataBlob {
        cb_data: data.len() as u32,
        pb_data: data.as_ptr() as *mut u8,
    };
    let mut blob_out = DataBlob {
        cb_data: 0,
        pb_data: ptr::null_mut(),
    };
    let ok = unsafe {
        if protect {
            CryptProtectData(
                &blob_in,
                ptr::null(),
                ptr::null(),
                ptr::null_mut(),
                ptr::null_mut(),
                0,
                &mut blob_out,
            )
        } else {
            CryptUnprotectData(
                &blob_in,
                ptr::null_mut(),
                ptr::null(),
                ptr::null_mut(),
                ptr::null_mut(),
                0,
                &mut blob_out,
            )
        }
    };
    if ok == 0 || blob_out.pb_data.is_null() {
        return None;
    }
    let out = unsafe {
        std::slice::from_raw_parts(blob_out.pb_data, blob_out.cb_data as usize).to_vec()
    };
    unsafe { LocalFree(blob_out.pb_data as *mut c_void) };
    Some(out)
}

#[cfg(windows)]
pub fn protect(data: &[u8]) -> Option<Vec<u8>> {
    dpapi(data, true)
}

#[cfg(windows)]
pub fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
    dpapi(data, false)
}

#[cfg(target_os = "linux")]
pub use crate::segredo::{protect, unprotect};

// ------------------------------------------------------------ settings ---

/// Current version of the settings file format.
pub const VERSAO_SETTINGS: u32 = 2;

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Settings {
    #[serde(default)]
    pub versao: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    #[serde(default)]
    pub contas: Vec<Conta>,
    /// Check for new versions automatically (absent = yes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub atualizacoes: Option<bool>,
    /// Interface language: "pt" or "en" (absent = the Windows one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idioma: Option<String>,
    /// When started by Windows at logon, stay in the tray (absent = yes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iniciar_minimizado: Option<bool>,
    /// Windows notification when a connection drops (absent = yes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avisos_de_queda: Option<bool>,
}

#[cfg(windows)]
fn appdata() -> PathBuf {
    PathBuf::from(std::env::var("APPDATA").unwrap_or_else(|_| ".".into()))
}

/// App data folder (settings and logs).
#[cfg(windows)]
pub fn app_dir() -> PathBuf {
    appdata().join("VPN")
}

/// The user's home folder.
#[cfg(target_os = "linux")]
pub fn pasta_home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

/// App data folder (settings and logs): $XDG_CONFIG_HOME/vpn, usually ~/.config/vpn.
#[cfg(target_os = "linux")]
pub fn app_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| pasta_home().join(".config"))
        .join("vpn")
}

/// Linux: contas.dat exists but could not be decrypted (keyring locked or
/// unavailable). Saving would replace the user's accounts with an empty
/// list, so nothing is written in this run.
#[cfg(target_os = "linux")]
static ILEGIVEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The saved accounts could not be read (Linux keyring unavailable).
pub fn contas_ilegiveis() -> bool {
    #[cfg(target_os = "linux")]
    {
        ILEGIVEL.load(std::sync::atomic::Ordering::SeqCst)
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

fn settings_file() -> PathBuf {
    app_dir().join("contas.dat")
}

fn ler_arquivo(caminho: &std::path::Path) -> Option<Settings> {
    let raw = std::fs::read(caminho).ok()?;
    let plain = unprotect(&raw)?;
    serde_json::from_slice(&plain).ok()
}

pub fn load_settings() -> Settings {
    let lido = ler_arquivo(&settings_file());
    #[cfg(target_os = "linux")]
    if lido.is_none() && settings_file().exists() {
        ILEGIVEL.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    let mut s = lido.unwrap_or_default();
    s.versao = VERSAO_SETTINGS;
    s
}

pub fn save_settings(settings: &Settings) {
    if contas_ilegiveis() {
        return;
    }
    let Ok(json) = serde_json::to_vec(settings) else {
        return;
    };
    let Some(enc) = protect(&json) else {
        return;
    };
    let _ = std::fs::create_dir_all(app_dir());
    // write to a temporary file and rename: a crash in the middle of the write
    // does not corrupt the accounts already saved
    let destino = settings_file();
    let temp = destino.with_extension("dat.tmp");
    if std::fs::write(&temp, enc).is_ok() {
        let _ = std::fs::rename(&temp, &destino);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::Autenticacao;

    #[test]
    fn roundtrip_dpapi() {
        let secreto = b"segredo-teste-123";
        let enc = protect(secreto).unwrap();
        assert_ne!(enc.as_slice(), secreto.as_slice());
        assert_eq!(unprotect(&enc).unwrap(), secreto);
    }

    #[test]
    fn formato_novo_preserva_varias_contas() {
        let mut s = Settings {
            versao: VERSAO_SETTINGS,
            theme: Some("light".into()),
            ..Default::default()
        };
        for nome in ["Trabalho", "Cliente X"] {
            let mut c = Conta::nova();
            c.nome = nome.into();
            c.autenticacao = Autenticacao::SenhaMaisToken;
            c.senha = "p@ss\"w".into();
            s.contas.push(c);
        }
        let json = serde_json::to_vec(&s).unwrap();
        // goes through the same path as the real file (DPAPI round trip)
        let volta: Settings =
            serde_json::from_slice(&unprotect(&protect(&json).unwrap()).unwrap()).unwrap();
        assert_eq!(volta.contas.len(), 2);
        assert_eq!(volta.contas[1].nome, "Cliente X");
        assert_eq!(volta.contas[1].senha, "p@ss\"w");
        assert_eq!(volta.contas[1].autenticacao, Autenticacao::SenhaMaisToken);
    }

    /// Test harness (screenshots on Linux): saves the settings described in
    /// the JSON file VPN_EXEMPLO_JSON through the app's own encryption, in the
    /// app folder of this environment (XDG_CONFIG_HOME / APPDATA).
    ///     VPN_EXEMPLO_JSON=contas.json cargo test -- --ignored grava_contas_de_exemplo
    #[test]
    #[ignore]
    fn grava_contas_de_exemplo() {
        let json = std::fs::read(std::env::var("VPN_EXEMPLO_JSON").expect("VPN_EXEMPLO_JSON")).unwrap();
        let s: Settings = serde_json::from_slice(&json).unwrap();
        save_settings(&s);
        assert_eq!(load_settings().contas.len(), s.contas.len());
    }

    #[test]
    fn atualizacao_automatica_ligada_por_padrao() {
        // files written before this option existed do not have the field
        let s: Settings = serde_json::from_str(r#"{"versao":2,"theme":"dark"}"#).unwrap();
        assert!(s.contas.is_empty());
        assert_eq!(s.atualizacoes, None);
        // when turned off, the choice is saved
        let s = Settings {
            atualizacoes: Some(false),
            ..Default::default()
        };
        assert!(serde_json::to_string(&s).unwrap().contains("\"atualizacoes\":false"));
    }
}
