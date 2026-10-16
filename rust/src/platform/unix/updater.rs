//! Linux and macOS side of the updater: HTTPS download (ureq with rustls,
//! honors the proxy environment variables), SHA-256 (sha2) and waiting for
//! the previous process. The asset name and what to take from it are in
//! <os>/updater.rs.

use crate::update::{ErroHttp, REPOSITORIO, VERSAO_ATUAL};
use std::path::Path;
use std::time::Duration;

/// The downloaded file needs the execute permission.
pub fn tornar_executavel(arquivo: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(arquivo, std::fs::Permissions::from_mode(0o755));
}

/// On the first start after an update: waits for the previous version to close
/// (it is still disconnecting the VPNs and holding the single instance).
pub fn aguardar_processo(pid: u32, limite: Duration) {
    let fim = std::time::Instant::now() + limite;
    while vivo(pid) && std::time::Instant::now() < fim {
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// The process exists (signal 0 only checks; EPERM = exists, another user's).
fn vivo(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    let existe = unsafe { libc::kill(pid, 0) } == 0;
    existe || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

pub fn sha256_hex(dados: &[u8]) -> Option<String> {
    use sha2::Digest;
    Some(
        sha2::Sha256::digest(dados)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    )
}

/// The Linux and macOS builds are not signed by a publisher.
pub fn assinante(_caminho: &Path) -> Option<String> {
    None
}

/// Simple GET (follows HTTPS redirects, like the one of the asset
/// downloads). `progresso` receives the bytes read so far.
pub fn http_get(
    url: &str,
    limite: usize,
    progresso: &mut dyn FnMut(u64),
) -> Result<Vec<u8>, ErroHttp> {
    use std::io::Read;
    if url::Url::parse(url).is_err() {
        return Err(ErroHttp::Url);
    }
    let agente = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(30))
        .try_proxy_from_env(true)
        .user_agent(&format!(
            "VPN/{VERSAO_ATUAL} (+https://github.com/{REPOSITORIO})"
        ))
        .build();
    let resposta = agente
        .get(url)
        .set(
            "Accept",
            "application/vnd.github+json, application/octet-stream",
        )
        .set("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(s, _) => ErroHttp::Status(u32::from(s)),
            ureq::Error::Transport(_) => ErroHttp::Rede(0),
        })?;
    let mut leitor = resposta.into_reader();
    let mut corpo = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let lidos = leitor.read(&mut buf).map_err(|_| ErroHttp::Rede(0))?;
        if lidos == 0 {
            break;
        }
        corpo.extend_from_slice(&buf[..lidos]);
        if corpo.len() > limite {
            return Err(ErroHttp::Grande);
        }
        progresso(corpo.len() as u64);
    }
    Ok(corpo)
}
