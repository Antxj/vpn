//! Linux: protection of the saved accounts (the role DPAPI plays on Windows).
//!
//! contas.dat is encrypted with AES-256-GCM. The key lives in the desktop
//! keyring (Secret Service: GNOME Keyring, KWallet), unlocked by the user's
//! login. Without a keyring service (minimal desktops), the key goes to a
//! file next to the accounts that only this user can read.
//!
//! The file header says where its key is, so a keyring that is momentarily
//! unavailable never makes the app create a new key for an existing file.

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// The keyring may be locked (or not running yet): the saved accounts are
/// then kept untouched until it opens (settings.rs).
pub const PODE_FICAR_INDISPONIVEL: bool = true;

const MAGICO: &[u8; 7] = b"VPNLNX1";
const SERVICO: &str = "io.github.antxj.vpn";
const NO_CHAVEIRO: u8 = b'K';
const EM_ARQUIVO: u8 = b'F';

/// Key already obtained in this run (the keyring is asked only once).
static CHAVE: Mutex<Option<(u8, [u8; 32])>> = Mutex::new(None);

/// Unit tests never touch the user's keyring or key file (a locked keyring
/// would hang them, and they would leave entries behind): they use a key
/// kept in memory. The exception is the harness that writes real sample
/// accounts (settings.rs, VPN_EXEMPLO_JSON).
fn chave_de_teste() -> Option<(u8, [u8; 32])> {
    if !cfg!(test) || std::env::var_os("VPN_EXEMPLO_JSON").is_some() {
        return None;
    }
    static TESTE: OnceLock<[u8; 32]> = OnceLock::new();
    Some((
        EM_ARQUIVO,
        *TESTE.get_or_init(|| nova_chave().expect("random key")),
    ))
}

fn usuario_do_chaveiro() -> String {
    match std::env::var("VPN_INSTANCE") {
        Ok(s) if !s.is_empty() => format!("contas-{s}"),
        _ => "contas".into(),
    }
}

fn arquivo_da_chave() -> PathBuf {
    crate::settings::app_dir().join(".chave")
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn de_hex(s: &str) -> Option<[u8; 32]> {
    let s = s.trim();
    if s.len() != 64 {
        return None;
    }
    let mut k = [0u8; 32];
    for (i, par) in s.as_bytes().chunks(2).enumerate() {
        k[i] = u8::from_str_radix(std::str::from_utf8(par).ok()?, 16).ok()?;
    }
    Some(k)
}

fn nova_chave() -> Option<[u8; 32]> {
    let mut k = [0u8; 32];
    getrandom::fill(&mut k).ok()?;
    Some(k)
}

fn chaveiro() -> Option<keyring::Entry> {
    if std::env::var_os("VPN_SEM_CHAVEIRO").is_some() {
        return None; // manual tests: the file fallback
    }
    keyring::Entry::new(SERVICO, &usuario_do_chaveiro()).ok()
}

/// Key stored at `origem`; creates it when `criar` (only for a new file).
fn chave_de(origem: u8, criar: bool) -> Option<[u8; 32]> {
    if let Some((o, k)) = chave_de_teste() {
        return (o == origem).then_some(k);
    }
    if let Some((o, k)) = *CHAVE.lock().unwrap() {
        if o == origem {
            return Some(k);
        }
    }
    let k = match origem {
        NO_CHAVEIRO => {
            let entrada = chaveiro()?;
            // started with the session, the keyring may not be ready yet:
            // tries for a few seconds before giving up. A slow failure means
            // the user answered the unlock prompt (cancelled): no new prompt.
            let mut tentativas = 0;
            loop {
                let inicio = std::time::Instant::now();
                let resposta = entrada.get_password();
                if resposta.is_err() && inicio.elapsed() > std::time::Duration::from_secs(2) {
                    tentativas = 10;
                }
                match resposta {
                    Ok(s) => break de_hex(&s)?,
                    Err(keyring::Error::NoEntry) if criar => {
                        let k = nova_chave()?;
                        entrada.set_password(&hex(&k)).ok()?;
                        break k;
                    }
                    Err(keyring::Error::NoEntry) => return None,
                    Err(_) if tentativas < 10 => {
                        tentativas += 1;
                        std::thread::sleep(std::time::Duration::from_millis(500));
                    }
                    Err(_) => return None,
                }
            }
        }
        _ => match std::fs::read_to_string(arquivo_da_chave()) {
            Ok(s) => de_hex(&s)?,
            Err(_) if criar => {
                let k = nova_chave()?;
                let _ = std::fs::create_dir_all(crate::settings::app_dir());
                let criado = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(arquivo_da_chave())
                    .and_then(|mut f| f.write_all(hex(&k).as_bytes()));
                match criado {
                    Ok(()) => k,
                    // created by another thread meanwhile: use that one
                    Err(_) => {
                        std::thread::sleep(std::time::Duration::from_millis(50));
                        de_hex(&std::fs::read_to_string(arquivo_da_chave()).ok()?)?
                    }
                }
            }
            Err(_) => return None,
        },
    };
    *CHAVE.lock().unwrap() = Some((origem, k));
    Some(k)
}

/// Where the key of a NEW file goes: the keyring when there is one.
/// None when there is no keyring and the key file cannot be created either.
fn origem_para_gravar() -> Option<(u8, [u8; 32])> {
    if let Some(teste) = chave_de_teste() {
        return Some(teste);
    }
    if let Some(ja) = *CHAVE.lock().unwrap() {
        return Some(ja);
    }
    if let Some(k) = chave_de(NO_CHAVEIRO, true) {
        return Some((NO_CHAVEIRO, k));
    }
    Some((EM_ARQUIVO, chave_de(EM_ARQUIVO, true)?))
}

pub fn protect(dados: &[u8]) -> Option<Vec<u8>> {
    let (origem, k) = origem_para_gravar()?;
    let mut nonce = [0u8; 12];
    getrandom::fill(&mut nonce).ok()?;
    let cifrado = Aes256Gcm::new_from_slice(&k)
        .ok()?
        .encrypt(&Nonce::from(nonce), dados)
        .ok()?;
    let mut saida = Vec::with_capacity(MAGICO.len() + 1 + 12 + cifrado.len());
    saida.extend_from_slice(MAGICO);
    saida.push(origem);
    saida.extend_from_slice(&nonce);
    saida.extend_from_slice(&cifrado);
    Some(saida)
}

pub fn unprotect(dados: &[u8]) -> Option<Vec<u8>> {
    let cabecalho = MAGICO.len() + 1 + 12;
    if dados.len() < cabecalho || !dados.starts_with(MAGICO) {
        return None;
    }
    let origem = dados[MAGICO.len()];
    let k = chave_de(origem, false)?;
    let nonce: [u8; 12] = dados[MAGICO.len() + 1..cabecalho].try_into().ok()?;
    Aes256Gcm::new_from_slice(&k)
        .ok()?
        .decrypt(&Nonce::from(nonce), &dados[cabecalho..])
        .ok()
}
