//! DPAPI (CryptProtectData) + persisted settings.
//!
//! %APPDATA%/VPN/contas.dat: a JSON encrypted with DPAPI in the user scope,
//! with the preferences and the list of accounts.

use crate::accounts::Conta;
use serde::{Deserialize, Serialize};
use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr;

#[repr(C)]
struct DataBlob {
    cb_data: u32,
    pb_data: *mut u8,
}

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

#[link(name = "kernel32")]
extern "system" {
    fn LocalFree(mem: *mut c_void) -> *mut c_void;
}

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

pub fn protect(data: &[u8]) -> Option<Vec<u8>> {
    dpapi(data, true)
}

pub fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
    dpapi(data, false)
}

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
}

fn appdata() -> PathBuf {
    PathBuf::from(std::env::var("APPDATA").unwrap_or_else(|_| ".".into()))
}

/// App data folder (settings and logs).
pub fn app_dir() -> PathBuf {
    appdata().join("VPN")
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
    let mut s = ler_arquivo(&settings_file()).unwrap_or_default();
    s.versao = VERSAO_SETTINGS;
    s
}

pub fn save_settings(settings: &Settings) {
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
