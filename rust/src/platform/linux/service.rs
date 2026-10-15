//! Linux: starts OpenVPN as root on behalf of the user, the role the
//! OpenVPN interactive service plays on Windows.
//!
//! - Once, an administrator authorizes the user (polkit asks for the admin
//!   password): this executable is copied to HELPER (root-owned), the user
//!   joins GROUP, and a polkit rule lets members of GROUP run HELPER with
//!   pkexec without a password.
//! - Each connection runs `pkexec HELPER --openvpn-helper <dir> <options>`.
//!   The helper (as root) accepts only the options the app itself uses,
//!   checks the .ovpn (no plugins, scripts or files outside its folder),
//!   starts openvpn with `--script-security 1` and passes its output back.
//!   When the app closes its standard input (stopping the connection, or the
//!   app ending/crashing), the helper stops openvpn: nothing is left behind.
//!
//! Same interface as the Windows module (src/service.rs).

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Root-owned copy of this executable that pkexec runs.
pub const HELPER: &str = "/usr/local/libexec/vpn-openvpn-helper";
/// Users authorized to start connections without a password.
const GRUPO: &str = "vpn-openvpn";
const ACAO_POLKIT: &str = "io.github.antxj.vpn.openvpn";
const POLITICA: &str = "/usr/share/polkit-1/actions/io.github.antxj.vpn.policy";
const REGRA_JS: &str = "/etc/polkit-1/rules.d/49-vpn-openvpn.rules";
const REGRA_PKLA: &str = "/etc/polkit-1/localauthority/50-local.d/49-vpn-openvpn.pkla";
/// Command line argument of the elevated step that authorizes a user.
pub const ARG_AUTHORIZE: &str = "--authorize-openvpn-user";
/// Command line argument of the helper run through pkexec.
pub const ARG_HELPER: &str = "--openvpn-helper";
/// Prints VERSAO_DO_AJUDANTE (no root needed): tells the app whether the
/// installed copy is current.
pub const ARG_VERSAO_DO_AJUDANTE: &str = "--openvpn-helper-version";
/// Version of the helper's rules. Raise it whenever the helper changes: the
/// app then asks for the one-time authorization again (installing the new
/// copy). App updates that do not touch the helper ask for nothing.
pub const VERSAO_DO_AJUDANTE: u32 = 1;
const PKEXEC: &str = "/usr/bin/pkexec";

/// Why OpenVPN could not be started.
#[derive(Debug, PartialEq)]
pub enum Falha {
    /// pkexec (polkit) is not installed.
    Indisponivel,
    /// The user needs to be authorized once.
    NaoAutorizado(String),
    Outra(String),
}

impl Falha {
    pub fn mensagem(&self) -> String {
        match self {
            Falha::Indisponivel => tr!(
                "O pkexec (polkit) não está instalado. Instale o pacote polkit da sua \
                 distribuição.",
                "pkexec (polkit) is not installed. Install your distribution's polkit package."
            )
            .into(),
            Falha::NaoAutorizado(m) | Falha::Outra(m) => m.clone(),
        }
    }
}

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

/// The current user already went through the one-time authorization, and
/// the installed helper is current.
fn autorizado() -> bool {
    Path::new(POLITICA).exists() && usuario_no_grupo(&usuario_atual(), GRUPO) && ajudante_atual()
}

/// Asks the installed copy its version (as the user, no root). Up to 3 s: a
/// copy that does not answer counts as outdated.
fn ajudante_atual() -> bool {
    let Ok(mut filho) = Command::new(HELPER)
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
    saida.trim() == VERSAO_DO_AJUDANTE.to_string()
}

fn usuario_no_grupo(usuario: &str, grupo: &str) -> bool {
    let (Ok(u), Ok(g)) = (
        std::ffi::CString::new(usuario),
        std::ffi::CString::new(grupo),
    ) else {
        return false;
    };
    unsafe {
        let gr = libc::getgrnam(g.as_ptr());
        let pw = libc::getpwnam(u.as_ptr());
        if gr.is_null() || pw.is_null() {
            return false;
        }
        let alvo = (*gr).gr_gid;
        let mut n: libc::c_int = 64;
        let mut grupos = vec![0 as libc::gid_t; n as usize];
        if libc::getgrouplist(u.as_ptr(), (*pw).pw_gid, grupos.as_mut_ptr(), &mut n) < 0 {
            grupos.resize(n.max(0) as usize, 0);
            if libc::getgrouplist(u.as_ptr(), (*pw).pw_gid, grupos.as_mut_ptr(), &mut n) < 0 {
                return false;
            }
        }
        grupos.truncate(n.max(0) as usize);
        grupos.contains(&alvo)
    }
}

/// Starts openvpn through the helper, in `workdir`, with `opcoes` (quoted
/// with `argumento`). `--log <file>` is handled here: the app writes the
/// helper's output to that file itself (root never writes in the user's folder).
pub fn iniciar(workdir: &Path, opcoes: &str) -> Result<Processo, Falha> {
    if !Path::new(PKEXEC).exists() {
        return Err(Falha::Indisponivel);
    }
    if !autorizado() {
        return Err(Falha::NaoAutorizado(
            tr!(
                "Esta conta ainda não foi autorizada a iniciar o OpenVPN.",
                "This account is not authorized to start OpenVPN yet."
            )
            .into(),
        ));
    }
    let mut args = dividir(opcoes);
    let mut log = None;
    if let Some(i) = args.iter().position(|a| a == "--log") {
        if i + 1 < args.len() {
            log = Some(PathBuf::from(args.remove(i + 1)));
        }
        args.remove(i);
    }
    let mut filho = Command::new(PKEXEC)
        .arg(HELPER)
        .arg(ARG_HELPER)
        .arg(workdir)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Falha::Outra(e.to_string()))?;

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

/// Windows cleans up openvpn.exe left behind by a crash; on Linux the helper
/// stops openvpn by itself when the app's end of the pipe closes.
pub fn encerrar_se_openvpn(_pid: u32) -> bool {
    false
}

pub fn grupo_autorizado() -> String {
    GRUPO.to_string()
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

/// Asks polkit for an administrator's approval to authorize the current
/// user once. Runs this same executable as root with ARG_AUTHORIZE.
pub fn autorizar_usuario_atual() -> Result<(), String> {
    if !Path::new(PKEXEC).exists() {
        return Err(Falha::Indisponivel.mensagem());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let status = Command::new(PKEXEC)
        .arg(exe)
        .arg(ARG_AUTHORIZE)
        .arg(usuario_atual())
        .status()
        .map_err(|e| e.to_string())?;
    match status.code() {
        Some(0) => Ok(()),
        Some(126) => Err(tr!("a autorização foi cancelada", "the authorization was cancelled").into()),
        Some(127) => Err(tr!(
            "o polkit não autorizou (senha de administrador incorreta ou sem agente de autenticação)",
            "polkit did not authorize (wrong administrator password or no authentication agent)"
        )
        .into()),
        Some(c) => Err(trf!(
            "não foi possível autorizar o usuário (código {c})",
            "could not authorize the user (code {c})"
        )),
        None => Err(tr!("não foi possível autorizar o usuário", "could not authorize the user").into()),
    }
}

// ============================================================== root side ==

/// The user that called pkexec (PKEXEC_UID), as (uid, name).
fn chamador() -> Option<(u32, String)> {
    let uid: u32 = std::env::var("PKEXEC_UID").ok()?.parse().ok()?;
    let nome = unsafe {
        let pw = libc::getpwuid(uid);
        if pw.is_null() {
            return None;
        }
        std::ffi::CStr::from_ptr((*pw).pw_name)
            .to_string_lossy()
            .into_owned()
    };
    Some((uid, nome))
}

fn gravar_root(caminho: &str, conteudo: &[u8], modo: u32) -> std::io::Result<()> {
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

fn politica_xml() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE policyconfig PUBLIC "-//freedesktop//DTD PolicyKit Policy Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/PolicyKit/1/policyconfig.dtd">
<policyconfig>
  <vendor>VPN</vendor>
  <vendor_url>https://github.com/Antxj/vpn</vendor_url>
  <action id="{ACAO_POLKIT}">
    <description>Start an OpenVPN connection for the VPN app</description>
    <message>Authentication is required to start a VPN connection</message>
    <defaults>
      <allow_any>no</allow_any>
      <allow_inactive>no</allow_inactive>
      <allow_active>auth_admin_keep</allow_active>
    </defaults>
    <annotate key="org.freedesktop.policykit.exec.path">{HELPER}</annotate>
  </action>
</policyconfig>
"#
    )
}

fn regra_js() -> String {
    format!(
        "// VPN app: members of {GRUPO} may start OpenVPN connections (local, active session)\n\
         polkit.addRule(function(action, subject) {{\n\
         \x20   if (action.id == \"{ACAO_POLKIT}\" && subject.isInGroup(\"{GRUPO}\") && subject.local && subject.active) {{\n\
         \x20       return polkit.Result.YES;\n\
         \x20   }}\n\
         }});\n"
    )
}

fn regra_pkla() -> String {
    format!(
        "[VPN app: start OpenVPN connections]\nIdentity=unix-group:{GRUPO}\nAction={ACAO_POLKIT}\n\
         ResultAny=no\nResultInactive=no\nResultActive=yes\n"
    )
}

fn executar(programa: &str, args: &[&str]) -> bool {
    Command::new(programa)
        .args(args)
        .status()
        .is_ok_and(|s| s.success())
}

/// Root side of `autorizar_usuario_atual` (run through pkexec): installs the
/// helper and the polkit rule, creates the group and adds the user to it.
/// Returns the process exit code.
pub fn adicionar_ao_grupo(usuario: &str) -> i32 {
    if unsafe { libc::geteuid() } != 0 {
        return 2;
    }
    // only the user who asked (and that an administrator approved)
    match chamador() {
        Some((_, nome)) if nome == usuario => {}
        _ => return 3,
    }
    let Ok(dados) = std::fs::read("/proc/self/exe") else {
        return 4;
    };
    if gravar_root(HELPER, &dados, 0o755).is_err() {
        return 5;
    }
    if !executar("groupadd", &["-f", GRUPO]) && !executar("/usr/sbin/groupadd", &["-f", GRUPO]) {
        return 6;
    }
    if !executar("usermod", &["-aG", GRUPO, usuario])
        && !executar("/usr/sbin/usermod", &["-aG", GRUPO, usuario])
    {
        return 7;
    }
    if gravar_root(POLITICA, politica_xml().as_bytes(), 0o644).is_err() {
        return 8;
    }
    // polkit 0.106+ reads JavaScript rules; older versions (Ubuntu 22.04),
    // .pkla files - both are written when their folder exists
    let mut alguma = false;
    if Path::new("/etc/polkit-1/rules.d").is_dir() {
        alguma |= gravar_root(REGRA_JS, regra_js().as_bytes(), 0o644).is_ok();
    }
    if Path::new("/etc/polkit-1/localauthority").is_dir() {
        alguma |= gravar_root(REGRA_PKLA, regra_pkla().as_bytes(), 0o644).is_ok();
    }
    if alguma {
        0
    } else {
        9
    }
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

fn openvpn_do_sistema() -> Option<&'static str> {
    [
        "/usr/sbin/openvpn",
        "/usr/bin/openvpn",
        "/usr/local/sbin/openvpn",
    ]
    .into_iter()
    .find(|p| std::fs::metadata(p).is_ok_and(|m| m.uid() == 0 && m.mode() & 0o022 == 0))
}

/// DNS of the VPN: the distribution's script for systemd-resolved, when installed.
fn script_de_dns() -> Option<&'static str> {
    [
        "/etc/openvpn/update-systemd-resolved",
        "/etc/openvpn/scripts/update-systemd-resolved",
    ]
    .into_iter()
    .find(|p| std::fs::metadata(p).is_ok_and(|m| m.uid() == 0 && m.mode() & 0o022 == 0))
}

/// The helper itself (as root, through pkexec). Returns the exit code.
pub fn executar_helper(args: &[String]) -> i32 {
    match helper(args) {
        Ok(codigo) => codigo,
        Err(e) => {
            // "ERROR:" makes the app show this line (vpn::erros_do_log)
            eprintln!("ERROR: vpn-openvpn-helper: {e}");
            1
        }
    }
}

fn helper(args: &[String]) -> Result<i32, String> {
    if unsafe { libc::geteuid() } != 0 {
        return Err("must run as root (through pkexec)".into());
    }
    let (uid, _) = chamador().ok_or("PKEXEC_UID")?;
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
    let dir = Path::new("/run/vpn-openvpn-helper");
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    let copia = dir.join(format!("{}.conf", std::process::id()));
    gravar_root(&copia.to_string_lossy(), texto.as_bytes(), 0o600).map_err(|e| e.to_string())?;

    let openvpn = openvpn_do_sistema().ok_or("openvpn is not installed")?;
    let mut cmd = Command::new(openvpn);
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
    match script_de_dns() {
        Some(script) => {
            cmd.args([
                "--script-security",
                "2",
                "--up",
                script,
                "--down",
                script,
                "--down-pre",
            ]);
        }
        None => {
            cmd.args(["--script-security", "1"]);
        }
    }
    let mut filho = cmd
        .stdin(Stdio::null())
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
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

/// Linux: OpenVPN creates its tun device by itself; nothing to create.
pub fn criar_adaptador(_openvpn: &Path, _hwid: &str) -> Result<(), String> {
    Err("tun".into())
}

/// Explanation of the one-time authorization (asked before running it).
pub fn pergunta_de_autorizacao() -> String {
    let grupo = grupo_autorizado();
    trf!(
        "Para conectar sem pedir a senha de administrador a cada vez, esta \
         conta do Linux precisa ser autorizada uma única vez (o app instala \
         um ajudante do OpenVPN e inclui a conta no grupo \"{grupo}\").\n\n\
         O sistema vai pedir a senha de um administrador agora. Continuar?",
        "To connect without asking for the administrator password every time, \
         this Linux account needs to be authorized once (the app installs an \
         OpenVPN helper and adds the account to the \"{grupo}\" group).\n\n\
         The system will ask for an administrator's password now. Continue?"
    )
}

/// The app run as a helper: the root helper started by pkexec, its version
/// check, or the one-time authorization (after the polkit password). Does
/// that job and exits.
pub fn tratar_argumentos_especiais() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some(ARG_HELPER) => std::process::exit(executar_helper(&args[2..])),
        Some(ARG_VERSAO_DO_AJUDANTE) => {
            println!("{VERSAO_DO_AJUDANTE}");
            std::process::exit(0);
        }
        _ => {}
    }
    if let Some(i) = args.iter().position(|a| a == ARG_AUTHORIZE) {
        let usuario = args.get(i + 1).cloned().unwrap_or_default();
        std::process::exit(adicionar_ao_grupo(&usuario));
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
        ] {
            assert!(
                validar_ovpn(&format!("client\n{ruim}\n")).is_err(),
                "{ruim}"
            );
        }
    }

    #[test]
    fn regras_do_polkit() {
        assert!(politica_xml().contains(HELPER));
        assert!(regra_js().contains(GRUPO) && regra_js().contains(ACAO_POLKIT));
        assert!(regra_pkla().contains("Identity=unix-group:vpn-openvpn"));
    }
}
