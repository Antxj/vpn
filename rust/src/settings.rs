//! Persisted settings: the preferences and the list of accounts, in
//! contas.dat (a JSON encrypted by the system: DPAPI on Windows, a key from
//! the desktop keyring on Linux - see platform/<os>/secret.rs). The folder
//! comes from platform/<os>/paths.rs.

use crate::accounts::Conta;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub use crate::platform::paths::app_dir;
pub use crate::platform::secret::{protect, unprotect};


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

/// contas.dat exists but could not be decrypted because the system's secret
/// store was unavailable (Linux keyring locked). Saving would replace the
/// user's accounts with an empty list, so nothing is written in this run.
static ILEGIVEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The saved accounts could not be read (secret store unavailable).
pub fn contas_ilegiveis() -> bool {
    ILEGIVEL.load(std::sync::atomic::Ordering::SeqCst)
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
    if crate::platform::secret::PODE_FICAR_INDISPONIVEL && lido.is_none() && settings_file().exists() {
        ILEGIVEL.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    let mut s = lido.unwrap_or_default();
    s.versao = VERSAO_SETTINGS;
    s
}

/// Reads the saved accounts again after they could not be read at start
/// (keyring unlocked meanwhile). Some(settings) when it worked.
pub fn tentar_ler_de_novo() -> Option<Settings> {
    let mut s = ler_arquivo(&settings_file())?;
    s.versao = VERSAO_SETTINGS;
    ILEGIVEL.store(false, std::sync::atomic::Ordering::SeqCst);
    Some(s)
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
