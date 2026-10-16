//! Updates through GitHub Releases, with no server of our own.
//!
//! - Unobtrusive check: 30 s after opening and then every 6 hours, the app
//!   queries `releases/latest` on the public GitHub API (which ignores
//!   pre-releases). Failures are silent. Nothing is sent besides the
//!   request itself (GitHub sees the IP and the version in the User-Agent).
//! - "Update now": downloads the release VPN.exe, verifies and replaces the
//!   executable, reopens the app and reconnects the VPNs that were on.
//!
//! Replacement safety:
//! - the downloaded file must have exactly the SHA-256 that GitHub publishes
//!   for it (the asset `digest` field);
//! - if the current executable is signed (Authenticode), the new one must
//!   have a valid signature FROM THE SAME publisher. From the first signed
//!   version on, an unsigned version is never installed.
//!
//! Download, SHA-256 and signature come from platform/<os>/updater.rs
//! (Windows: WinHTTP, BCrypt, WinVerifyTrust; Linux: ureq and sha2 - the
//! Linux build is not signed, so only the SHA-256 check applies).

pub use crate::platform::updater::{aguardar_processo, ARQUIVO};
use crate::platform::updater::{
    assinante, depois_de_trocar, extrair, http_get, sha256_hex, tornar_executavel, CABECALHO,
};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

pub const REPOSITORIO: &str = "Antxj/vpn";
pub const VERSAO_ATUAL: &str = env!("CARGO_PKG_VERSION");
const PRIMEIRA_VERIFICACAO: Duration = Duration::from_secs(30);
/// Every 6 hours: an app that stays open all the time (started with
/// Windows) still hears about a new version on the same day.
const INTERVALO: Duration = Duration::from_secs(6 * 60 * 60);
const LIMITE_JSON: usize = 2 * 1024 * 1024;
const LIMITE_EXE: usize = 64 * 1024 * 1024;

/// A published version, newer than the current one.
#[derive(Clone, Debug, PartialEq)]
pub struct Versao {
    pub numero: String,
    /// Release page (what's new).
    pub pagina: String,
    /// VPN.exe link; empty if the release does not have the asset.
    pub download: String,
    pub tamanho: u64,
    /// SHA-256 published by GitHub (lowercase hex).
    pub sha256: Option<String>,
}

impl Versao {
    /// Can be installed by the app (otherwise, only from the page).
    pub fn instalavel(&self) -> bool {
        !self.download.is_empty() && self.sha256.is_some()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Estado {
    Nada,
    /// Check requested by the user, in progress.
    Verificando,
    /// Result of the check requested by the user.
    EmDia,
    FalhaVerificacao(String),
    Disponivel(Versao),
    /// Downloading (fraction from 0 to 1).
    Baixando(Versao, f32),
    FalhaInstalacao(Versao, String),
    /// Installed: the app is closing to open the new version.
    Reiniciando(Versao),
}

static ESTADO: Mutex<Estado> = Mutex::new(Estado::Nada);
type AoAcharVersao = Box<dyn Fn(&str) + Send + Sync>;

static AVISO: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();
static NOVA_VERSAO: OnceLock<AoAcharVersao> = OnceLock::new();
/// Last version announced by `ao_achar_versao` (each one only once).
static ANUNCIADA: Mutex<Option<String>> = Mutex::new(None);

/// Function called when the automatic check finds a new version (once per
/// version): the app shows a Windows notification.
pub fn ao_achar_versao(f: impl Fn(&str) + Send + Sync + 'static) {
    let _ = NOVA_VERSAO.set(Box::new(f));
}

/// True only the first time this version is seen.
fn primeira_vez(numero: &str) -> bool {
    let mut anunciada = ANUNCIADA.lock().unwrap();
    if anunciada.as_deref() == Some(numero) {
        return false;
    }
    *anunciada = Some(numero.to_string());
    true
}

/// Function called on every state change (to repaint the UI).
pub fn ao_mudar(f: impl Fn() + Send + Sync + 'static) {
    let _ = AVISO.set(Box::new(f));
}

pub fn estado() -> Estado {
    ESTADO.lock().unwrap().clone()
}

fn definir(e: Estado) {
    *ESTADO.lock().unwrap() = e;
    if let Some(f) = AVISO.get() {
        f();
    }
}

/// Number of the new version, while there is one to offer.
pub fn disponivel() -> Option<String> {
    match estado() {
        Estado::Disponivel(v)
        | Estado::Baixando(v, _)
        | Estado::FalhaInstalacao(v, _)
        | Estado::Reiniciando(v) => Some(v.numero),
        _ => None,
    }
}

/// Hides the result of the manual check (when leaving the screen).
pub fn limpar_resultado() {
    if matches!(estado(), Estado::EmDia | Estado::FalhaVerificacao(_)) {
        definir(Estado::Nada);
    }
}

pub fn pagina_repositorio() -> String {
    format!("https://github.com/{REPOSITORIO}")
}

pub fn pagina_releases() -> String {
    format!("https://github.com/{REPOSITORIO}/releases")
}

/// VPN_UPDATE_URL changes the queried address (tests only).
fn url_api() -> String {
    std::env::var("VPN_UPDATE_URL")
        .unwrap_or_else(|_| format!("https://api.github.com/repos/{REPOSITORIO}/releases/latest"))
}

/// Automatic check, while `ativa()` says yes.
pub fn iniciar_verificacao_periodica(ativa: fn() -> bool) {
    std::thread::spawn(move || {
        std::thread::sleep(PRIMEIRA_VERIFICACAO);
        loop {
            if ativa() {
                verificar(false);
            }
            std::thread::sleep(INTERVALO);
        }
    });
}

/// Check requested by the user: shows the result, failures included.
pub fn verificar_agora() {
    std::thread::spawn(|| verificar(true));
}

fn verificar(manual: bool) {
    if matches!(
        estado(),
        Estado::Verificando | Estado::Baixando(..) | Estado::Reiniciando(_)
    ) {
        return;
    }
    if manual {
        definir(Estado::Verificando);
    }
    match consultar(&url_api(), VERSAO_ATUAL) {
        Ok(Some(v)) => {
            let numero = v.numero.clone();
            definir(Estado::Disponivel(v));
            if !manual && primeira_vez(&numero) {
                if let Some(f) = NOVA_VERSAO.get() {
                    f(&numero);
                }
            }
        }
        Ok(None) if manual => definir(Estado::EmDia),
        Err(e) if manual => definir(Estado::FalhaVerificacao(e)),
        _ => {}
    }
}

fn consultar(url: &str, atual: &str) -> Result<Option<Versao>, String> {
    let corpo = http_get(url, LIMITE_JSON, &mut |_| {}).map_err(|e| match e {
        // private/non-existent repository or no published release
        ErroHttp::Status(404) => tr!(
            "Nenhuma versão publicada foi encontrada.",
            "No published version was found."
        )
        .to_string(),
        outro => outro.mensagem(),
    })?;
    interpretar(&corpo, atual)
}

/// Reads the `releases/latest` response; Some if newer than `atual`.
fn interpretar(corpo: &[u8], atual: &str) -> Result<Option<Versao>, String> {
    let inesperada = || {
        tr!(
            "Resposta inesperada do GitHub.",
            "Unexpected response from GitHub."
        )
        .to_string()
    };
    let v: serde_json::Value = serde_json::from_slice(corpo).map_err(|_| inesperada())?;
    if v["draft"].as_bool() == Some(true) || v["prerelease"].as_bool() == Some(true) {
        return Ok(None);
    }
    let tag = v["tag_name"].as_str().ok_or_else(inesperada)?;
    let numero = tag.trim_start_matches(['v', 'V']).to_string();
    if !versao_maior(&numero, atual) {
        return Ok(None);
    }
    let pagina = v["html_url"]
        .as_str()
        .map(String::from)
        .unwrap_or_else(pagina_releases);
    let anexo = v["assets"].as_array().and_then(|lista| {
        lista.iter().find(|a| {
            a["name"]
                .as_str()
                .is_some_and(|n| n.eq_ignore_ascii_case(ARQUIVO))
        })
    });
    let mut versao = Versao {
        numero,
        pagina,
        download: String::new(),
        tamanho: 0,
        sha256: None,
    };
    if let Some(a) = anexo {
        versao.download = a["browser_download_url"]
            .as_str()
            .unwrap_or_default()
            .into();
        versao.tamanho = a["size"].as_u64().unwrap_or(0);
        versao.sha256 = a["digest"]
            .as_str()
            .and_then(|d| d.strip_prefix("sha256:"))
            .filter(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()))
            .map(|h| h.to_ascii_lowercase());
    }
    Ok(Some(versao))
}

/// "1.2.3" > "1.2.2"? Versions with a suffix ("1.3.0-rc1") are never
/// offered: pre-releases are left to those who download from the page.
fn versao_maior(nova: &str, atual: &str) -> bool {
    fn partes(s: &str) -> Option<[u64; 3]> {
        let mut out = [0u64; 3];
        let mut n = 0;
        for p in s.trim().split('.') {
            if n == 3 {
                return None;
            }
            out[n] = p.parse().ok()?;
            n += 1;
        }
        (n > 0).then_some(out)
    }
    match (
        partes(nova),
        partes(atual.split(['-', '+']).next().unwrap_or(atual)),
    ) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

// ------------------------------------------------------------- install --

/// Downloads, verifies and installs the version in the background; then reopens the app.
pub fn instalar(v: Versao) {
    if matches!(estado(), Estado::Baixando(..) | Estado::Reiniciando(_)) {
        return;
    }
    definir(Estado::Baixando(v.clone(), 0.0));
    std::thread::spawn(move || {
        let alvo = match std::env::current_exe() {
            Ok(p) => p,
            Err(e) => {
                definir(Estado::FalhaInstalacao(
                    v,
                    trf!(
                        "Não encontrei o executável atual: {e}",
                        "Could not find the current executable: {e}"
                    ),
                ));
                return;
            }
        };
        let mut ultimo = 0u32;
        let resultado = baixar_e_trocar(&v, &alvo, &mut |fracao| {
            // repaint on every 1% (not on every downloaded chunk)
            let pct = (fracao * 100.0) as u32;
            if pct != ultimo {
                ultimo = pct;
                definir(Estado::Baixando(v.clone(), fracao));
            }
        });
        match resultado {
            Ok(()) => {
                definir(Estado::Reiniciando(v.clone()));
                if let Err(e) = reabrir(&alvo) {
                    definir(Estado::FalhaInstalacao(
                        v,
                        trf!(
                            "A versão nova foi instalada, mas não consegui reabrir o \
                             aplicativo ({e}). Feche e abra de novo.",
                            "The new version was installed, but the app could not be \
                             reopened ({e}). Close it and open it again."
                        ),
                    ));
                }
            }
            Err(e) => definir(Estado::FalhaInstalacao(v, e)),
        }
    });
}

/// `VPN.exe` -> `VPN.exe.<sufixo>`, in the same folder (same disk: the swap
/// is a simple rename).
fn vizinho(alvo: &Path, sufixo: &str) -> PathBuf {
    let nome = alvo
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| ARQUIVO.into());
    alvo.with_file_name(format!("{nome}.{sufixo}"))
}

fn baixar_e_trocar(v: &Versao, alvo: &Path, progresso: &mut dyn FnMut(f32)) -> Result<(), String> {
    if v.download.is_empty() {
        return Err(trf!(
            "A versão {} não tem o {ARQUIVO} anexado.",
            "Version {} does not have {ARQUIVO} attached.",
            v.numero
        ));
    }
    let esperado = v.sha256.as_deref().ok_or_else(|| {
        tr!(
            "O GitHub não informou o SHA-256 deste arquivo, então ele não pode ser \
             conferido. Baixe pela página da versão.",
            "GitHub did not provide the SHA-256 of this file, so it cannot be \
             verified. Download it from the release page."
        )
        .to_string()
    })?;
    let total = v.tamanho;
    let dados = http_get(&v.download, LIMITE_EXE, &mut |lidos| {
        if total > 0 {
            progresso((lidos as f32 / total as f32).min(1.0));
        }
    })
    .map_err(|e| trf!("Falha no download: {}", "Download failed: {}", e.mensagem()))?;

    if total > 0 && dados.len() as u64 != total {
        return Err(tr!(
            "O download veio incompleto. Nada foi alterado.",
            "The download is incomplete. Nothing was changed."
        )
        .into());
    }
    if sha256_hex(&dados).as_deref() != Some(esperado) {
        return Err(tr!(
            "O arquivo baixado não confere com o publicado (SHA-256 diferente). \
             Nada foi alterado.",
            "The downloaded file does not match the published one (different \
             SHA-256). Nothing was changed."
        )
        .into());
    }
    // macOS: the executable inside the .zip (elsewhere the file itself)
    let dados = extrair(dados)?;
    if !dados.starts_with(CABECALHO) {
        return Err(tr!(
            "O arquivo baixado não é um executável. Nada foi alterado.",
            "The downloaded file is not an executable. Nothing was changed."
        )
        .into());
    }

    let novo = vizinho(alvo, "novo");
    let antigo = vizinho(alvo, "antigo");
    std::fs::write(&novo, &dados).map_err(|e| {
        trf!(
            "Não consegui gravar a versão nova na pasta do aplicativo: {e}",
            "Could not write the new version to the app folder: {e}"
        )
    })?;
    tornar_executavel(&novo);

    // from the first signed version on, only the same publisher is accepted
    if let Some(editor) = assinante(alvo) {
        if assinante(&novo).as_deref() != Some(editor.as_str()) {
            let _ = std::fs::remove_file(&novo);
            return Err(trf!(
                "O arquivo baixado não tem a assinatura digital de \"{editor}\". \
                 Nada foi alterado.",
                "The downloaded file is not digitally signed by \"{editor}\". \
                 Nothing was changed."
            ));
        }
    }

    // Windows allows renaming the running executable (not deleting it): the
    // current one becomes .antigo, deleted on the next start
    let _ = std::fs::remove_file(&antigo);
    let falha = |e: std::io::Error| {
        trf!(
            "Não consegui substituir o aplicativo: {e}",
            "Could not replace the app: {e}"
        )
    };
    if let Err(e) = std::fs::rename(alvo, &antigo) {
        let _ = std::fs::remove_file(&novo);
        return Err(falha(e));
    }
    if let Err(e) = std::fs::rename(&novo, alvo) {
        let _ = std::fs::rename(&antigo, alvo);
        let _ = std::fs::remove_file(&novo);
        return Err(falha(e));
    }
    depois_de_trocar(alvo);
    Ok(())
}

/// Opens the new version (which waits for this process to end) and exits this
/// one, disconnecting first. The new version turns the connected accounts back on.
fn reabrir(alvo: &Path) -> std::io::Result<()> {
    let m = crate::engine::get();
    let ligadas: Vec<String> = m
        .contas()
        .into_iter()
        .filter(|c| m.ativa(&c.id))
        .map(|c| c.id)
        .collect();
    let mut cmd = std::process::Command::new(alvo);
    cmd.arg(ARG_APOS_ATUALIZAR)
        .arg(std::process::id().to_string());
    if !ligadas.is_empty() {
        cmd.arg(ARG_RECONECTAR).arg(ligadas.join(","));
    }
    cmd.spawn()?;
    m.encerrar()
}

pub const ARG_APOS_ATUALIZAR: &str = "--apos-atualizar";
pub const ARG_RECONECTAR: &str = "--reconectar";

/// On the first start after an update: waits for the previous version to close
/// (it is still disconnecting the VPNs and holding the single instance).
/// Deletes leftovers of an update (the previous executable).
pub fn limpar_restos() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(vizinho(&exe, "antigo"));
        let _ = std::fs::remove_file(vizinho(&exe, "novo"));
    }
}

/// Download failure (the system's HTTP code fills it in).
#[derive(Debug, PartialEq)]
pub enum ErroHttp {
    /// Network/TLS failure (WinHTTP code).
    Rede(u32),
    Status(u32),
    Grande,
    Url,
}

impl ErroHttp {
    fn mensagem(&self) -> String {
        match self {
            ErroHttp::Rede(_) => tr!(
                "Sem conexão com o GitHub. Verifique a internet.",
                "Could not reach GitHub. Check the internet connection."
            )
            .into(),
            ErroHttp::Status(s) => {
                trf!(
                    "O GitHub respondeu com erro (HTTP {s}).",
                    "GitHub returned an error (HTTP {s})."
                )
            }
            ErroHttp::Grande => tr!("Resposta grande demais.", "Response too large.").into(),
            ErroHttp::Url => tr!("Endereço inválido.", "Invalid address.").into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn comparacao_de_versoes() {
        assert!(versao_maior("1.0.1", "1.0.0"));
        assert!(versao_maior("1.1", "1.0.9"));
        assert!(versao_maior("2.0.0", "1.12.3"));
        assert!(versao_maior("1.0.10", "1.0.9")); // numeric, not alphabetical
        assert!(!versao_maior("1.0.0", "1.0.0"));
        assert!(!versao_maior("0.9.9", "1.0.0"));
        // pre-releases and garbage are never offered
        assert!(!versao_maior("1.1.0-rc1", "1.0.0"));
        assert!(!versao_maior("abc", "1.0.0"));
        assert!(!versao_maior("", "1.0.0"));
        assert!(!versao_maior("1.2.3.4", "1.0.0"));
    }

    fn release(tag: &str, anexos: &str) -> Vec<u8> {
        format!(
            r#"{{"tag_name":"{tag}","html_url":"https://github.com/x/y/releases/tag/{tag}",
               "draft":false,"prerelease":false,"assets":[{anexos}]}}"#
        )
        .into_bytes()
    }

    const HASH: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn le_a_release_do_github() {
        let anexo = format!(
            r#"{{"name":"LEIA-ME.txt","browser_download_url":"https://x/leia","size":10}},
               {{"name":"{ARQUIVO}","browser_download_url":"https://x/VPN.exe","size":123,
                 "digest":"sha256:{}"}}"#,
            HASH.to_uppercase()
        );
        let v = interpretar(&release("v1.0.1", &anexo), "1.0.0")
            .unwrap()
            .unwrap();
        assert_eq!(v.numero, "1.0.1");
        assert_eq!(v.download, "https://x/VPN.exe");
        assert_eq!(v.tamanho, 123);
        assert_eq!(v.sha256.as_deref(), Some(HASH));
        assert!(v.instalavel());
        assert!(v.pagina.ends_with("/v1.0.1"));

        // same or older version: nothing to offer
        assert_eq!(
            interpretar(&release("v1.0.0", &anexo), "1.0.0").unwrap(),
            None
        );
        // without the published digest, only from the page
        let sem_hash = format!(
            r#"{{"name":"{ARQUIVO}","browser_download_url":"https://x/VPN.exe","size":1}}"#
        );
        let sem_hash = sem_hash.as_str();
        let v = interpretar(&release("v2.0.0", sem_hash), "1.0.0")
            .unwrap()
            .unwrap();
        assert!(!v.instalavel());
        // without the asset, only from the page
        let v = interpretar(&release("v2.0.0", ""), "1.0.0")
            .unwrap()
            .unwrap();
        assert!(!v.instalavel());
        // a pre-release is never offered
        let pre = String::from_utf8(release("v9.0.0", &anexo))
            .unwrap()
            .replace(r#""prerelease":false"#, r#""prerelease":true"#);
        assert_eq!(interpretar(pre.as_bytes(), "1.0.0").unwrap(), None);
        assert!(interpretar(b"<html>", "1.0.0").is_err());
    }

    #[test]
    fn sha256_pelo_windows() {
        assert_eq!(sha256_hex(b"abc").as_deref(), Some(HASH));
    }

    /// Minimal HTTP server on 127.0.0.1: answers each known path.
    fn servidor(rotas: Vec<(&'static str, Vec<u8>)>) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                let mut s = s;
                let mut req = [0u8; 4096];
                let n = s.read(&mut req).unwrap_or(0);
                let linha = String::from_utf8_lossy(&req[..n]);
                let caminho = linha.split_whitespace().nth(1).unwrap_or("").to_string();
                let resp = match rotas.iter().find(|(c, _)| *c == caminho) {
                    Some((_, corpo)) => {
                        let mut r = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            corpo.len()
                        )
                        .into_bytes();
                        r.extend_from_slice(corpo);
                        r
                    }
                    None => {
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_vec()
                    }
                };
                let _ = s.write_all(&resp);
            }
        });
        base
    }

    #[test]
    fn baixa_confere_e_troca_o_executavel() {
        let cabecalho = CABECALHO;
        let novo_exe = [cabecalho, b" versao nova do aplicativo"].concat();
        let hash = sha256_hex(&novo_exe).unwrap();
        let base = servidor(vec![("/VPN.exe", novo_exe.clone())]);

        let pasta =
            std::env::temp_dir().join(format!("vpn-teste-atualizacao-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&pasta);
        std::fs::create_dir_all(&pasta).unwrap();
        let alvo = pasta.join("VPN.exe");

        let versao = |sha: &str| Versao {
            numero: "9.9.9".into(),
            pagina: String::new(),
            download: format!("{base}/VPN.exe"),
            tamanho: novo_exe.len() as u64,
            sha256: Some(sha.into()),
        };

        // hash different from the published one: refuses and touches nothing
        std::fs::write(&alvo, b"MZ versao atual").unwrap();
        let errado = "0".repeat(64);
        let e = baixar_e_trocar(&versao(&errado), &alvo, &mut |_| {}).unwrap_err();
        assert!(e.contains("SHA-256"), "{e}");
        assert_eq!(std::fs::read(&alvo).unwrap(), b"MZ versao atual");
        assert!(!vizinho(&alvo, "novo").exists());

        // right hash: swaps, keeps the previous one as .antigo and reports progress
        let mut fracoes = Vec::new();
        baixar_e_trocar(&versao(&hash), &alvo, &mut |f| fracoes.push(f)).unwrap();
        assert_eq!(std::fs::read(&alvo).unwrap(), novo_exe);
        assert_eq!(
            std::fs::read(vizinho(&alvo, "antigo")).unwrap(),
            b"MZ versao atual"
        );
        assert!(!vizinho(&alvo, "novo").exists());
        assert_eq!(fracoes.last().copied(), Some(1.0));

        // asset missing on the server
        let mut sumiu = versao(&hash);
        sumiu.download = format!("{base}/nao-existe");
        assert!(baixar_e_trocar(&sumiu, &alvo, &mut |_| {}).is_err());

        let _ = std::fs::remove_dir_all(&pasta);
    }

    #[test]
    fn consulta_a_api_por_http() {
        let corpo = release("v9.9.9", "");
        let base = servidor(vec![("/latest", corpo)]);
        let v = consultar(&format!("{base}/latest"), "1.0.0")
            .unwrap()
            .unwrap();
        assert_eq!(v.numero, "9.9.9");
        let e = consultar(&format!("{base}/outro"), "1.0.0").unwrap_err();
        assert!(e.contains("Nenhuma versão publicada"), "{e}");
    }
}
