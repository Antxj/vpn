//! Linux and macOS: the root helper that starts OpenVPN on behalf of the
//! user, the role the OpenVPN interactive service plays on Windows.
//!
//! The app runs `<elevator> HELPER --openvpn-helper <dir> <options>`
//! (Linux: pkexec; macOS: sudo with a rule for HELPER), where HELPER is a
//! root-owned copy of this executable installed by the one-time
//! authorization. The helper (as root) accepts only the options the app
//! itself uses, checks the .ovpn (no plugins, scripts or files outside its
//! folder), starts openvpn and passes its output back. When the app closes
//! its standard input (stopping the connection, or the app ending/crashing),
//! the helper stops openvpn: nothing is left behind.
//!
//! What differs between the systems (how to elevate, how the user is
//! authorized, which openvpn, DNS) is in <os>/service.rs.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Command line argument of the helper run as root.
pub const ARG_HELPER: &str = "--openvpn-helper";
/// Prints the helper's version (no root needed): tells the app whether the
/// installed copy is current.
pub const ARG_VERSAO_DO_AJUDANTE: &str = "--openvpn-helper-version";
/// Command line argument of the elevated step that authorizes a user.
pub const ARG_AUTHORIZE: &str = "--authorize-openvpn-user";

// ================================================================ app side ==

/// openvpn started through the helper. The app follows the helper process;
/// closing its standard input makes the helper stop openvpn.
pub struct Processo {
    pub pid: u32,
    filho: Mutex<Child>,
    entrada: Mutex<Option<ChildStdin>>,
}

impl Processo {
    pub fn acompanhavel(&self) -> bool {
        true
    }

    /// Exit code, if the process has ended.
    pub fn terminou(&self) -> Option<u32> {
        match self.filho.lock().unwrap().try_wait() {
            Ok(Some(status)) => Some(status.code().map(|c| c as u32).unwrap_or(u32::MAX)),
            Ok(None) => None,
            Err(_) => Some(u32::MAX),
        }
    }

    /// Waits up to `limite` for the process to end; stops it afterwards.
    pub fn esperar(&self, limite: Duration) -> Option<u32> {
        let fim = Instant::now() + limite;
        while Instant::now() < fim {
            if let Some(c) = self.terminou() {
                return Some(c);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        self.matar();
        let fim = Instant::now() + Duration::from_secs(8);
        while Instant::now() < fim {
            if let Some(c) = self.terminou() {
                return Some(c);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        self.terminou()
    }

    /// Closes the helper's standard input: it stops openvpn and exits.
    pub fn matar(&self) {
        self.entrada.lock().unwrap().take();
    }
}

/// Quotes one argument so `dividir` reads it back (paths with spaces).
pub fn argumento(a: &str) -> String {
    if !a.is_empty() && !a.contains([' ', '\t', '"', '\\']) {
        return a.to_string();
    }
    let mut out = String::from('"');
    for c in a.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// Splits the options string built with `argumento`.
fn dividir(texto: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut atual = String::new();
    let mut tem = false;
    let mut aspas = false;
    let mut chars = texto.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                aspas = !aspas;
                tem = true;
            }
            '\\' if aspas => {
                if let Some(n) = chars.next() {
                    atual.push(n);
                }
            }
            c if c.is_whitespace() && !aspas => {
                if tem {
                    args.push(std::mem::take(&mut atual));
                    tem = false;
                }
            }
            c => {
                atual.push(c);
                tem = true;
            }
        }
    }
    if tem {
        args.push(atual);
    }
    args
}

/// Asks the installed copy at `helper` its version (as the user, no root).
/// Up to 3 s: a copy that does not answer counts as outdated.
pub fn versao_confere(helper: &str, versao: u32) -> bool {
    let Ok(mut filho) = Command::new(helper)
        .arg(ARG_VERSAO_DO_AJUDANTE)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let fim = Instant::now() + Duration::from_secs(3);
    loop {
        match filho.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < fim => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = filho.kill();
                let _ = filho.wait();
                return false;
            }
        }
    }
    let mut saida = String::new();
    if let Some(mut s) = filho.stdout.take() {
        let _ = s.read_to_string(&mut saida);
    }
    saida.trim() == versao.to_string()
}

pub fn usuario_atual() -> String {
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if !pw.is_null() {
            return std::ffi::CStr::from_ptr((*pw).pw_name)
                .to_string_lossy()
                .into_owned();
        }
    }
    std::env::var("USER").unwrap_or_default()
}

/// Starts `programa` with `prefixo` (the elevator and the helper with its
/// arguments), then `workdir` and `opcoes` (quoted with `argumento`).
/// `--log <file>` is handled here: the app writes the helper's output to
/// that file itself (root never writes in the user's folder).
pub fn iniciar(
    programa: &str,
    prefixo: &[&str],
    workdir: &Path,
    opcoes: &str,
) -> Result<Processo, String> {
    let mut args = dividir(opcoes);
    let mut log = None;
    if let Some(i) = args.iter().position(|a| a == "--log") {
        if i + 1 < args.len() {
            log = Some(PathBuf::from(args.remove(i + 1)));
        }
        args.remove(i);
    }
    let mut filho = Command::new(programa)
        .args(prefixo)
        .arg(workdir)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;

    // the helper's output (openvpn's log) goes to the log file, like --log
    let arquivo = log.and_then(|p| std::fs::File::create(p).ok());
    let saida = std::sync::Arc::new(Mutex::new(arquivo));
    for leitor in [
        filho
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
        filho
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    {
        let saida = saida.clone();
        std::thread::spawn(move || {
            for linha in BufReader::new(leitor).lines().map_while(Result::ok) {
                if let Some(f) = saida.lock().unwrap().as_mut() {
                    let _ = writeln!(f, "{linha}");
                }
            }
        });
    }
    let entrada = filho.stdin.take();
    Ok(Processo {
        pid: filho.id(),
        filho: Mutex::new(filho),
        entrada: Mutex::new(entrada),
    })
}

// ============================================================== root side ==

/// Name of a user id (None when the user does not exist).
pub fn nome_do_usuario(uid: u32) -> Option<String> {
    unsafe {
        let pw = libc::getpwuid(uid);
        if pw.is_null() {
            return None;
        }
        Some(
            std::ffi::CStr::from_ptr((*pw).pw_name)
                .to_string_lossy()
                .into_owned(),
        )
    }
}

/// Writes a root-owned file atomically (temporary file, then rename).
pub fn gravar_root(caminho: &str, conteudo: &[u8], modo: u32) -> std::io::Result<()> {
    if let Some(pai) = Path::new(caminho).parent() {
        std::fs::create_dir_all(pai)?;
    }
    let temp = format!("{caminho}.tmp");
    let _ = std::fs::remove_file(&temp);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(modo)
        .open(&temp)?;
    f.write_all(conteudo)?;
    f.sync_all()?;
    std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(modo))?;
    std::fs::rename(&temp, caminho)
}

/// Owned by root and not writable by anyone else: safe to run as root.
pub fn do_root_e_protegido(caminho: &str) -> bool {
    std::fs::metadata(caminho).is_ok_and(|m| m.uid() == 0 && m.mode() & 0o022 == 0)
}

/// Options the helper accepts (exactly what vpn.rs passes) and how many
/// values each one takes.
fn validar_opcoes(args: &[String]) -> Result<Vec<String>, String> {
    let ipv4 = |s: &str| s.parse::<std::net::Ipv4Addr>().is_ok();
    let numero = |s: &str| !s.is_empty() && s.len() <= 6 && s.chars().all(|c| c.is_ascii_digit());
    let mut saida = Vec::new();
    let mut i = 0;
    let mut configs = 0;
    while i < args.len() {
        let opcao = args[i].as_str();
        let valores: &[String] = &args[i + 1..];
        let n = match opcao {
            "--config" => {
                configs += 1;
                1
            }
            "--management" => {
                let porta_ok = valores
                    .get(1)
                    .is_some_and(|p| numero(p) && p.parse::<u16>().is_ok());
                if valores.first().map(String::as_str) != Some("127.0.0.1") || !porta_ok {
                    return Err("--management".into());
                }
                2
            }
            "--auth-retry" => {
                if valores.first().map(String::as_str) != Some("interact") {
                    return Err("--auth-retry".into());
                }
                1
            }
            "--connect-retry" => {
                if !valores.first().is_some_and(|v| numero(v)) {
                    return Err("--connect-retry".into());
                }
                1
            }
            "--route" => {
                let ok = valores.len() >= 3
                    && ipv4(&valores[0])
                    && ipv4(&valores[1])
                    && valores[2] == "net_gateway";
                if !ok {
                    return Err("--route".into());
                }
                3
            }
            "--management-query-passwords" | "--auth-nocache" | "--disable-dco" => 0,
            outra => return Err(format!("option not allowed: {outra}")),
        };
        if valores.len() < n {
            return Err(format!("{opcao}: missing value"));
        }
        saida.extend_from_slice(&args[i..=i + n]);
        i += n + 1;
    }
    if configs != 1 {
        return Err("exactly one --config".into());
    }
    Ok(saida)
}

/// Directives a .ovpn may not have when run as root: they run programs,
/// load code, write files or read the password from a file.
const PROIBIDAS: &[&str] = &[
    "plugin",
    "iproute",
    "config",
    "script-security",
    "up",
    "down",
    "route-up",
    "route-pre-down",
    "ipchange",
    "learn-address",
    "client-connect",
    "client-disconnect",
    "tls-verify",
    "auth-user-pass-verify",
    "tls-export-cert",
    "log",
    "log-append",
    "status",
    "writepid",
    "daemon",
    "chroot",
    "cd",
    "tmp-dir",
    "dev-node",
    "replay-persist",
    "ifconfig-pool-persist",
    "syslog",
    "askpass",
    "http-proxy-user-pass",
    "management",
    "management-client-user",
    "management-client-group",
    "management-hold",
    "management-external-key",
    "management-external-cert",
    "dns-updown",
];

/// Directives whose (first) argument is a file read by openvpn.
const COM_ARQUIVO: &[&str] = &[
    "ca",
    "cert",
    "key",
    "pkcs12",
    "tls-auth",
    "tls-crypt",
    "tls-crypt-v2",
    "dh",
    "extra-certs",
    "secret",
    "crl-verify",
];

/// Checks the .ovpn text. Returns the files it references (relative paths).
fn validar_ovpn(texto: &str) -> Result<Vec<String>, String> {
    let mut arquivos = Vec::new();
    let mut bloco: Option<String> = None;
    for linha in texto.lines() {
        let linha = linha.trim();
        if let Some(b) = &bloco {
            if linha.eq_ignore_ascii_case(&format!("</{b}>")) {
                bloco = None;
            }
            continue;
        }
        if linha.is_empty() || linha.starts_with('#') || linha.starts_with(';') {
            continue;
        }
        if let Some(tag) = linha.strip_prefix('<').and_then(|l| l.strip_suffix('>')) {
            if !tag.starts_with('/') {
                bloco = Some(tag.to_string());
            }
            continue;
        }
        let partes: Vec<&str> = linha.split_whitespace().collect();
        let diretiva = partes[0].trim_start_matches("--").to_ascii_lowercase();
        if PROIBIDAS.contains(&diretiva.as_str()) || diretiva.starts_with("management-") {
            return Err(format!("directive not allowed in the .ovpn: {diretiva}"));
        }
        // the password comes from the app, never from a file read by root
        if diretiva == "auth-user-pass" && partes.len() > 1 {
            return Err("auth-user-pass with a file".into());
        }
        if (diretiva == "http-proxy" || diretiva == "socks-proxy")
            && partes.get(3).is_some_and(|a| !a.starts_with("auto"))
        {
            return Err(format!("{diretiva} with a credentials file"));
        }
        if COM_ARQUIVO.contains(&diretiva.as_str()) {
            let arquivo = partes.get(1).map(|a| a.trim_matches('"')).unwrap_or("");
            if !arquivo.is_empty() && arquivo != "[inline]" {
                let p = Path::new(arquivo);
                if p.as_os_str().is_empty()
                    || !p.components().all(|c| matches!(c, Component::Normal(_)))
                {
                    return Err(format!(
                        "{diretiva}: file outside the .ovpn folder ({arquivo})"
                    ));
                }
                arquivos.push(arquivo.to_string());
            }
        }
    }
    Ok(arquivos)
}

/// A file the user owns (or root), not a symbolic link, inside `pasta`.
fn arquivo_do_usuario(caminho: &Path, pasta: &Path, uid: u32) -> Result<(), String> {
    let meta =
        std::fs::symlink_metadata(caminho).map_err(|e| format!("{}: {e}", caminho.display()))?;
    if !meta.file_type().is_file() {
        return Err(format!("{}: not a regular file", caminho.display()));
    }
    if meta.uid() != uid && meta.uid() != 0 {
        return Err(format!("{}: belongs to another user", caminho.display()));
    }
    let real = caminho.canonicalize().map_err(|e| e.to_string())?;
    if !real.starts_with(pasta) {
        return Err(format!("{}: outside the .ovpn folder", caminho.display()));
    }
    Ok(())
}

/// How this system runs openvpn as root.
pub struct Execucao<'a> {
    /// The user that asked (from the elevator: PKEXEC_UID or SUDO_UID).
    pub uid: u32,
    /// Root-owned openvpn executable.
    pub openvpn: &'a str,
    /// Root-only folder for the checked copy of the .ovpn.
    pub pasta_privada: &'a Path,
    /// Options added by the helper itself (script security, DNS).
    pub extras: Vec<String>,
    /// PATH of openvpn's environment.
    pub path: &'a str,
}

/// The helper's work (as root): checks the options and the .ovpn, runs
/// openvpn and waits for it. `args` = the .ovpn folder followed by the
/// options. Returns openvpn's exit code.
pub fn executar_openvpn(args: &[String], e: Execucao) -> Result<i32, String> {
    if unsafe { libc::geteuid() } != 0 {
        return Err("must run as root".into());
    }
    let uid = e.uid;
    let (pasta, opcoes) = args.split_first().ok_or("missing folder")?;
    let opcoes = validar_opcoes(opcoes)?;
    let i = opcoes
        .iter()
        .position(|a| a == "--config")
        .ok_or("--config")?;
    let config = PathBuf::from(&opcoes[i + 1]);
    let pasta = Path::new(pasta).canonicalize().map_err(|e| e.to_string())?;
    if config
        .parent()
        .map(Path::new)
        .and_then(|p| p.canonicalize().ok())
        .as_deref()
        != Some(pasta.as_path())
    {
        return Err("the .ovpn must be in the given folder".into());
    }
    arquivo_do_usuario(&config, &pasta, uid)?;
    let texto = std::fs::read(&config).map_err(|e| e.to_string())?;
    if texto.len() > 1024 * 1024 {
        return Err(".ovpn too large".into());
    }
    let texto = String::from_utf8_lossy(&texto).into_owned();
    for arquivo in validar_ovpn(&texto)? {
        arquivo_do_usuario(&pasta.join(arquivo), &pasta, uid)?;
    }

    // openvpn reads a root-owned copy: the user cannot change it after the check
    let dir = e.pasta_privada;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    let copia = dir.join(format!("{}.conf", std::process::id()));
    gravar_root(&copia.to_string_lossy(), texto.as_bytes(), 0o600).map_err(|e| e.to_string())?;

    let mut cmd = Command::new(e.openvpn);
    cmd.current_dir(&pasta).arg("--config").arg(&copia);
    let mut j = 0;
    while j < opcoes.len() {
        if opcoes[j] == "--config" {
            j += 2;
            continue;
        }
        cmd.arg(&opcoes[j]);
        j += 1;
    }
    cmd.args(&e.extras);
    let mut filho = cmd
        .stdin(Stdio::null())
        .env_clear()
        .env("PATH", e.path)
        .spawn()
        .map_err(|e| e.to_string())?;
    let pid = filho.id() as libc::pid_t;

    // the app closed its end (stopped, ended or crashed): stop openvpn
    std::thread::spawn(move || {
        let mut buf = [0u8; 64];
        while std::io::stdin().read(&mut buf).is_ok_and(|n| n > 0) {}
        unsafe { libc::kill(pid, libc::SIGTERM) };
        std::thread::sleep(Duration::from_secs(6));
        unsafe { libc::kill(pid, libc::SIGKILL) };
    });
    let status = filho.wait().map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&copia);
    Ok(status?.code().unwrap_or(1))
}

/// Exit code of the helper; errors go to the app's log.
pub fn codigo_de_saida(resultado: Result<i32, String>) -> i32 {
    match resultado {
        Ok(codigo) => codigo,
        Err(e) => {
            // "ERROR:" makes the app show this line (vpn::erros_do_log)
            eprintln!("ERROR: vpn-openvpn-helper: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Vec<String> {
        dividir(s)
    }

    #[test]
    fn argumentos_vao_e_voltam() {
        for a in [
            "simples",
            "/home/maria/Minhas VPNs/x.ovpn",
            "aspas \"e\" barra \\ fim",
            "",
        ] {
            assert_eq!(
                dividir(&format!("--config {}", argumento(a))),
                ["--config", a]
            );
        }
    }

    #[test]
    fn so_as_opcoes_do_app() {
        let ok = v(
            "--config /home/m/a.ovpn --management 127.0.0.1 7505 --management-query-passwords \
                    --auth-retry interact --auth-nocache --connect-retry 5 \
                    --route 200.1.2.3 255.255.255.255 net_gateway --disable-dco",
        );
        assert_eq!(validar_opcoes(&ok).unwrap(), ok);
        for ruim in [
            "--config a --config b",
            "--management 0.0.0.0 7505 --config a",
            "--config a --up /tmp/x.sh",
            "--config a --script-security 3",
            "--config a --route 1.2.3.4 255.255.255.255 vpn_gateway",
            "--config a --log /etc/passwd",
            "--config a --connect-retry 5;id",
            "--management 127.0.0.1",
            "--config a --dns-updown /tmp/x.sh",
        ] {
            assert!(validar_opcoes(&v(ruim)).is_err(), "{ruim}");
        }
    }

    #[test]
    fn ovpn_perigoso_e_recusado() {
        let normal = "client\ndev tun\nremote vpn.exemplo 1194\nca ca.crt\nkey \"chaves/meu.key\"\n\
                      auth-user-pass\n<tls-crypt>\nup /tmp/dentro-do-bloco-nao-conta\n</tls-crypt>\n\
                      # plugin comentado.so\n;up x\nhttp-proxy proxy 3128 auto\n";
        assert_eq!(validar_ovpn(normal).unwrap(), ["ca.crt", "chaves/meu.key"]);
        for ruim in [
            "plugin /tmp/x.so",
            "up /tmp/x.sh",
            "script-security 2",
            "iproute /tmp/fake-ip",
            "config /etc/outro.conf",
            "log /etc/passwd",
            "auth-user-pass /etc/shadow",
            "ca /etc/ssl/private/x.pem",
            "key ../../root/k.pem",
            "management-external-key",
            "http-proxy p 3128 /root/senha",
            "--up /tmp/x",
            "dns-updown /tmp/x.sh",
        ] {
            assert!(
                validar_ovpn(&format!("client\n{ruim}\n")).is_err(),
                "{ruim}"
            );
        }
    }
}
