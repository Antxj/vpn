//! Updates through GitHub Releases, with no server of our own.
//!
//! - Unobtrusive check: 30 s after opening and then once a day, the app
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
//! Everything through Windows' own APIs (WinHTTP, BCrypt, WinVerifyTrust):
//! no new dependency, and WinHTTP honors the system proxy.

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

pub const REPOSITORIO: &str = "Antxj/vpn";
pub const VERSAO_ATUAL: &str = env!("CARGO_PKG_VERSION");
/// Name of the release asset that contains the app.
const ARQUIVO: &str = "VPN.exe";
const PRIMEIRA_VERIFICACAO: Duration = Duration::from_secs(30);
const INTERVALO: Duration = Duration::from_secs(24 * 60 * 60);
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
static AVISO: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

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
        Ok(Some(v)) => definir(Estado::Disponivel(v)),
        Ok(None) if manual => definir(Estado::EmDia),
        Err(e) if manual => definir(Estado::FalhaVerificacao(e)),
        _ => {}
    }
}

fn consultar(url: &str, atual: &str) -> Result<Option<Versao>, String> {
    let corpo = http_get(url, LIMITE_JSON, &mut |_| {}).map_err(|e| match e {
        // private/non-existent repository or no published release
        ErroHttp::Status(404) => {
            tr!("Nenhuma versão publicada foi encontrada.", "No published version was found.")
                .to_string()
        }
        outro => outro.mensagem(),
    })?;
    interpretar(&corpo, atual)
}

/// Reads the `releases/latest` response; Some if newer than `atual`.
fn interpretar(corpo: &[u8], atual: &str) -> Result<Option<Versao>, String> {
    let inesperada = || tr!("Resposta inesperada do GitHub.", "Unexpected response from GitHub.").to_string();
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
        versao.download = a["browser_download_url"].as_str().unwrap_or_default().into();
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
    match (partes(nova), partes(atual.split(['-', '+']).next().unwrap_or(atual))) {
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
                    trf!("Não encontrei o executável atual: {e}", "Could not find the current executable: {e}"),
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

fn baixar_e_trocar(
    v: &Versao,
    alvo: &Path,
    progresso: &mut dyn FnMut(f32),
) -> Result<(), String> {
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
    if !dados.starts_with(b"MZ") {
        return Err(tr!(
            "O arquivo baixado não é um executável. Nada foi alterado.",
            "The downloaded file is not an executable. Nothing was changed."
        )
        .into());
    }

    let novo = vizinho(alvo, "novo");
    let antigo = vizinho(alvo, "antigo");
    std::fs::write(&novo, &dados)
        .map_err(|e| {
            trf!(
                "Não consegui gravar a versão nova na pasta do aplicativo: {e}",
                "Could not write the new version to the app folder: {e}"
            )
        })?;

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
        trf!("Não consegui substituir o aplicativo: {e}", "Could not replace the app: {e}")
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
    cmd.arg(ARG_APOS_ATUALIZAR).arg(std::process::id().to_string());
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
pub fn aguardar_processo(pid: u32, limite: Duration) {
    const SYNCHRONIZE: u32 = 0x0010_0000;
    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(acesso: u32, herdar: i32, pid: u32) -> *mut c_void;
        fn WaitForSingleObject(h: *mut c_void, ms: u32) -> u32;
        fn CloseHandle(h: *mut c_void) -> i32;
    }
    unsafe {
        let h = OpenProcess(SYNCHRONIZE, 0, pid);
        if !h.is_null() {
            WaitForSingleObject(h, limite.as_millis() as u32);
            CloseHandle(h);
        }
    }
}

/// Deletes leftovers of an update (the previous executable).
pub fn limpar_restos() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(vizinho(&exe, "antigo"));
        let _ = std::fs::remove_file(vizinho(&exe, "novo"));
    }
}

// ------------------------------------------------------- SHA-256 (BCrypt) --

fn sha256_hex(dados: &[u8]) -> Option<String> {
    use windows_sys::Win32::Security::Cryptography::{BCryptHash, BCRYPT_SHA256_ALG_HANDLE};
    let mut saida = [0u8; 32];
    let status = unsafe {
        BCryptHash(
            BCRYPT_SHA256_ALG_HANDLE,
            ptr::null(),
            0,
            dados.as_ptr(),
            u32::try_from(dados.len()).ok()?,
            saida.as_mut_ptr(),
            saida.len() as u32,
        )
    };
    (status == 0).then(|| saida.iter().map(|b| format!("{b:02x}")).collect())
}

// ----------------------------------------------- signature (WinVerifyTrust) --

/// Publisher name, if the file has a valid Authenticode signature.
pub fn assinante(caminho: &Path) -> Option<String> {
    use windows_sys::Win32::Security::Cryptography::{
        CertGetNameStringW, CERT_NAME_SIMPLE_DISPLAY_TYPE,
    };
    use windows_sys::Win32::Security::WinTrust::*;

    let caminho_w = crate::single::wide(&caminho.to_string_lossy());
    let mut arquivo = WINTRUST_FILE_INFO {
        cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: caminho_w.as_ptr(),
        hFile: ptr::null_mut(),
        pgKnownSubject: ptr::null_mut(),
    };
    let mut dados: WINTRUST_DATA = unsafe { std::mem::zeroed() };
    dados.cbStruct = std::mem::size_of::<WINTRUST_DATA>() as u32;
    dados.dwUIChoice = WTD_UI_NONE;
    dados.fdwRevocationChecks = WTD_REVOKE_WHOLECHAIN;
    dados.dwProvFlags = WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT;
    dados.dwUnionChoice = WTD_CHOICE_FILE;
    dados.Anonymous.pFile = &mut arquivo;
    dados.dwStateAction = WTD_STATEACTION_VERIFY;

    let mut acao = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    let sem_janela = -1isize as *mut c_void; // INVALID_HANDLE_VALUE: never shows UI
    let resultado = unsafe {
        WinVerifyTrust(sem_janela, &mut acao, &mut dados as *mut _ as *mut c_void)
    };

    let mut nome = None;
    if resultado == 0 {
        unsafe {
            let prov = WTHelperProvDataFromStateData(dados.hWVTStateData);
            let sgnr = if prov.is_null() {
                ptr::null_mut()
            } else {
                WTHelperGetProvSignerFromChain(prov, 0, 0, 0)
            };
            let cert = if sgnr.is_null() {
                ptr::null_mut()
            } else {
                WTHelperGetProvCertFromChain(sgnr, 0)
            };
            if !cert.is_null() && !(*cert).pCert.is_null() {
                let mut buf = [0u16; 256];
                let n = CertGetNameStringW(
                    (*cert).pCert,
                    CERT_NAME_SIMPLE_DISPLAY_TYPE,
                    0,
                    ptr::null(),
                    buf.as_mut_ptr(),
                    buf.len() as u32,
                );
                if n > 1 {
                    nome = Some(String::from_utf16_lossy(&buf[..n as usize - 1]));
                }
            }
        }
    }
    dados.dwStateAction = WTD_STATEACTION_CLOSE;
    unsafe {
        WinVerifyTrust(sem_janela, &mut acao, &mut dados as *mut _ as *mut c_void);
    }
    nome.filter(|n| !n.is_empty())
}

// ------------------------------------------------------------ HTTP (WinHTTP) --

#[derive(Debug, PartialEq)]
enum ErroHttp {
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
                trf!("O GitHub respondeu com erro (HTTP {s}).", "GitHub returned an error (HTTP {s}).")
            }
            ErroHttp::Grande => tr!("Resposta grande demais.", "Response too large.").into(),
            ErroHttp::Url => tr!("Endereço inválido.", "Invalid address.").into(),
        }
    }
}

/// Closes the WinHTTP handle when it goes out of scope.
struct Handle(*mut c_void);
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { windows_sys::Win32::Networking::WinHttp::WinHttpCloseHandle(self.0) };
        }
    }
}

fn ultimo_erro() -> u32 {
    unsafe { windows_sys::Win32::Foundation::GetLastError() }
}

/// Simple GET (follows HTTPS redirects, like the one of the asset
/// downloads). `progresso` receives the bytes read so far.
fn http_get(url: &str, limite: usize, progresso: &mut dyn FnMut(u64)) -> Result<Vec<u8>, ErroHttp> {
    use windows_sys::Win32::Networking::WinHttp::*;

    let url_w: Vec<u16> = url.encode_utf16().collect();
    let mut partes: URL_COMPONENTS = unsafe { std::mem::zeroed() };
    partes.dwStructSize = std::mem::size_of::<URL_COMPONENTS>() as u32;
    // -1: returns pointers into the URL itself
    partes.dwSchemeLength = u32::MAX;
    partes.dwHostNameLength = u32::MAX;
    partes.dwUrlPathLength = u32::MAX;
    partes.dwExtraInfoLength = u32::MAX;
    if unsafe { WinHttpCrackUrl(url_w.as_ptr(), url_w.len() as u32, 0, &mut partes) } == 0 {
        return Err(ErroHttp::Url);
    }
    let fatia = |p: *const u16, n: u32| -> Vec<u16> {
        if p.is_null() {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(p, n as usize) }.to_vec()
        }
    };
    let mut host = fatia(partes.lpszHostName, partes.dwHostNameLength);
    host.push(0);
    let mut caminho = fatia(partes.lpszUrlPath, partes.dwUrlPathLength);
    caminho.extend(fatia(partes.lpszExtraInfo, partes.dwExtraInfoLength));
    if caminho.is_empty() {
        caminho.push(b'/' as u16);
    }
    caminho.push(0);
    let https = partes.nScheme == WINHTTP_INTERNET_SCHEME_HTTPS;

    let agente = crate::single::wide(&format!("VPN/{VERSAO_ATUAL} (+https://github.com/{REPOSITORIO})"));
    let sessao = Handle(unsafe {
        WinHttpOpen(
            agente.as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            ptr::null(),
            ptr::null(),
            0,
        )
    });
    if sessao.0.is_null() {
        return Err(ErroHttp::Rede(ultimo_erro()));
    }
    unsafe { WinHttpSetTimeouts(sessao.0, 15_000, 15_000, 30_000, 30_000) };

    let conexao = Handle(unsafe { WinHttpConnect(sessao.0, host.as_ptr(), partes.nPort, 0) });
    if conexao.0.is_null() {
        return Err(ErroHttp::Rede(ultimo_erro()));
    }
    let verbo = crate::single::wide("GET");
    let pedido = Handle(unsafe {
        WinHttpOpenRequest(
            conexao.0,
            verbo.as_ptr(),
            caminho.as_ptr(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            if https { WINHTTP_FLAG_SECURE } else { 0 },
        )
    });
    if pedido.0.is_null() {
        return Err(ErroHttp::Rede(ultimo_erro()));
    }
    let cabecalhos = crate::single::wide(
        "Accept: application/vnd.github+json, application/octet-stream\r\n\
         X-GitHub-Api-Version: 2022-11-28\r\n",
    );
    let ok = unsafe {
        WinHttpSendRequest(pedido.0, cabecalhos.as_ptr(), u32::MAX, ptr::null(), 0, 0, 0) != 0
            && WinHttpReceiveResponse(pedido.0, ptr::null_mut()) != 0
    };
    if !ok {
        return Err(ErroHttp::Rede(ultimo_erro()));
    }

    let mut status: u32 = 0;
    let mut tam = std::mem::size_of::<u32>() as u32;
    unsafe {
        WinHttpQueryHeaders(
            pedido.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            ptr::null(),
            &mut status as *mut u32 as *mut c_void,
            &mut tam,
            ptr::null_mut(),
        )
    };
    if status != 200 {
        return Err(ErroHttp::Status(status));
    }

    let mut corpo = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let mut lidos: u32 = 0;
        let ok = unsafe {
            WinHttpReadData(
                pedido.0,
                buf.as_mut_ptr() as *mut c_void,
                buf.len() as u32,
                &mut lidos,
            )
        };
        if ok == 0 {
            return Err(ErroHttp::Rede(ultimo_erro()));
        }
        if lidos == 0 {
            break;
        }
        corpo.extend_from_slice(&buf[..lidos as usize]);
        if corpo.len() > limite {
            return Err(ErroHttp::Grande);
        }
        progresso(corpo.len() as u64);
    }
    Ok(corpo)
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
               {{"name":"VPN.exe","browser_download_url":"https://x/VPN.exe","size":123,
                 "digest":"sha256:{}"}}"#,
            HASH.to_uppercase()
        );
        let v = interpretar(&release("v1.0.1", &anexo), "1.0.0").unwrap().unwrap();
        assert_eq!(v.numero, "1.0.1");
        assert_eq!(v.download, "https://x/VPN.exe");
        assert_eq!(v.tamanho, 123);
        assert_eq!(v.sha256.as_deref(), Some(HASH));
        assert!(v.instalavel());
        assert!(v.pagina.ends_with("/v1.0.1"));

        // same or older version: nothing to offer
        assert_eq!(interpretar(&release("v1.0.0", &anexo), "1.0.0").unwrap(), None);
        // without the published digest, only from the page
        let sem_hash = r#"{"name":"VPN.exe","browser_download_url":"https://x/VPN.exe","size":1}"#;
        let v = interpretar(&release("v2.0.0", sem_hash), "1.0.0").unwrap().unwrap();
        assert!(!v.instalavel());
        // without the asset, only from the page
        let v = interpretar(&release("v2.0.0", ""), "1.0.0").unwrap().unwrap();
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
                    None => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_vec(),
                };
                let _ = s.write_all(&resp);
            }
        });
        base
    }

    #[test]
    fn baixa_confere_e_troca_o_executavel() {
        let novo_exe = b"MZ versao nova do aplicativo".to_vec();
        let hash = sha256_hex(&novo_exe).unwrap();
        let base = servidor(vec![("/VPN.exe", novo_exe.clone())]);

        let pasta = std::env::temp_dir().join(format!("vpn-teste-atualizacao-{}", std::process::id()));
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
        assert_eq!(std::fs::read(vizinho(&alvo, "antigo")).unwrap(), b"MZ versao atual");
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
        let v = consultar(&format!("{base}/latest"), "1.0.0").unwrap().unwrap();
        assert_eq!(v.numero, "9.9.9");
        let e = consultar(&format!("{base}/outro"), "1.0.0").unwrap_err();
        assert!(e.contains("Nenhuma versão publicada"), "{e}");
    }

    #[test]
    fn assinatura_digital() {
        // unsigned file
        let p = std::env::temp_dir().join(format!("vpn-teste-assinatura-{}.exe", std::process::id()));
        std::fs::write(&p, b"MZ nada assinado").unwrap();
        assert_eq!(assinante(&p), None);
        let _ = std::fs::remove_file(&p);

        // the official MSI embedded in the release is signed by OpenVPN Inc.
        let msi = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets").join("openvpn.msi");
        if msi.exists() {
            let nome = assinante(&msi).expect("the OpenVPN MSI should be signed");
            assert!(nome.contains("OpenVPN"), "{nome}");
        }
    }
}
