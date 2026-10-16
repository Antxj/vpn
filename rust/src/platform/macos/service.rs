//! macOS: starts OpenVPN as root on behalf of the user, the role the
//! OpenVPN interactive service plays on Windows (the helper itself is in
//! unix/helper.rs).
//!
//! - Once, an administrator authorizes the user (macOS asks for the admin
//!   password): this executable is copied to HELPER, the OpenVPN embedded in
//!   it to OPENVPN and OpenVPN's DNS script to SCRIPT_DNS, all root-owned in
//!   /Library/PrivilegedHelperTools; a sudo rule (REGRA_SUDO) lets this user
//!   run HELPER --openvpn-helper without a password - and nothing else.
//! - Each connection runs `sudo -n HELPER --openvpn-helper <dir> <options>`.
//!
//! Nothing runs as root from a place the user can write (not Homebrew, not
//! /usr/local), so no program of the user can take the helper's place.

use crate::platform::unix::helper::{self, Execucao};
use std::path::Path;
use std::process::{Command, Stdio};

pub use helper::{
    argumento, usuario_atual, Processo, ARG_AUTHORIZE, ARG_HELPER, ARG_VERSAO_DO_AJUDANTE,
};

/// Root-owned folder of the helper and its OpenVPN (macOS's place for
/// privileged helper tools).
const PASTA: &str = "/Library/PrivilegedHelperTools";
/// Root-owned copy of this executable that sudo runs.
pub const HELPER: &str = "/Library/PrivilegedHelperTools/io.github.antxj.vpn.helper";
/// Root-owned copy of the OpenVPN embedded in the app.
pub const OPENVPN: &str = "/Library/PrivilegedHelperTools/io.github.antxj.vpn.openvpn";
/// Root-owned copy of OpenVPN's DNS script for macOS.
const SCRIPT_DNS: &str = "/Library/PrivilegedHelperTools/io.github.antxj.vpn.dns-updown";
/// sudo rule: one line per authorized user. sudo ignores files in
/// sudoers.d whose name has a dot, so the temporary copy is ".vpn-openvpn".
const REGRA_SUDO: &str = "/etc/sudoers.d/vpn-openvpn";
const REGRA_SUDO_TEMPORARIA: &str = "/etc/sudoers.d/.vpn-openvpn";
const SUDO: &str = "/usr/bin/sudo";
/// Version of the helper's rules. Raise it whenever the helper changes: the
/// app then asks for the one-time authorization again (installing the new
/// copy). A new embedded OpenVPN asks for it by itself (see `autorizado`).
pub const VERSAO_DO_AJUDANTE: u32 = 1;

/// Why OpenVPN could not be started.
#[derive(Debug, PartialEq)]
pub enum Falha {
    /// This build has no OpenVPN inside.
    Indisponivel,
    /// The user needs to be authorized once.
    NaoAutorizado(String),
    Outra(String),
}

impl Falha {
    pub fn mensagem(&self) -> String {
        match self {
            Falha::Indisponivel => tr!(
                "Esta cópia do aplicativo foi gerada sem o OpenVPN. Baixe a versão \
                 publicada na página do projeto.",
                "This copy of the app was built without OpenVPN. Download the \
                 published version from the project page."
            )
            .into(),
            Falha::NaoAutorizado(m) | Falha::Outra(m) => m.clone(),
        }
    }
}

// ================================================================ app side ==

/// The installed copy is current: the helper's version, the same OpenVPN and
/// DNS script the app brings, and the sudo rule for this user.
fn autorizado() -> bool {
    helper::versao_confere(HELPER, VERSAO_DO_AJUDANTE)
        && std::fs::read(OPENVPN).is_ok_and(|b| b == super::openvpn::EMBUTIDO)
        && std::fs::read(SCRIPT_DNS).is_ok_and(|b| b == super::openvpn::SCRIPT_DE_DNS)
        && sudo_permite()
}

/// sudo lets this user run the helper without a password (`-l` only asks).
fn sudo_permite() -> bool {
    Command::new(SUDO)
        .args(["-n", "-l", HELPER, ARG_HELPER, "/"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Starts openvpn through the helper, in `workdir`, with `opcoes` (quoted
/// with `argumento`).
pub fn iniciar(workdir: &Path, opcoes: &str) -> Result<Processo, Falha> {
    if super::openvpn::EMBUTIDO.is_empty() {
        return Err(Falha::Indisponivel);
    }
    if !autorizado() {
        return Err(Falha::NaoAutorizado(
            tr!(
                "Este usuário do Mac ainda não foi autorizado a iniciar o OpenVPN.",
                "This Mac user is not authorized to start OpenVPN yet."
            )
            .into(),
        ));
    }
    helper::iniciar(SUDO, &["-n", HELPER, ARG_HELPER], workdir, opcoes).map_err(Falha::Outra)
}

/// Windows cleans up openvpn.exe left behind by a crash; here the helper
/// stops openvpn by itself when the app's end of the pipe closes.
pub fn encerrar_se_openvpn(_pid: u32) -> bool {
    false
}

/// AppleScript string literal.
fn texto_applescript(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Asks macOS for an administrator's approval (its own password window) to
/// authorize the current user once. Runs this same executable as root with
/// ARG_AUTHORIZE.
pub fn autorizar_usuario_atual() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let usuario = usuario_atual();
    let aviso = tr!(
        "O VPN quer instalar o OpenVPN e um ajudante para conectar sem pedir a \
         senha a cada vez.",
        "VPN wants to install OpenVPN and a helper to connect without asking \
         for the password every time."
    );
    let script = format!(
        "do shell script (quoted form of {}) & \" {ARG_AUTHORIZE} \" & (quoted form of {}) \
         with prompt {} with administrator privileges",
        texto_applescript(&exe.to_string_lossy()),
        texto_applescript(&usuario),
        texto_applescript(aviso)
    );
    let saida = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if saida.status.success() {
        return Ok(());
    }
    let erro = String::from_utf8_lossy(&saida.stderr);
    if erro.contains("-128") {
        return Err(tr!(
            "a autorização foi cancelada",
            "the authorization was cancelled"
        )
        .into());
    }
    Err(trf!(
        "não foi possível autorizar o usuário ({})",
        "could not authorize the user ({})",
        erro.trim()
    ))
}

// ============================================================== root side ==

/// macOS user names: letters, digits, ".", "_" and "-" (safe in a sudo rule).
fn nome_valido(usuario: &str) -> bool {
    !usuario.is_empty()
        && !usuario.starts_with('-')
        && usuario.len() <= 64
        && usuario
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// The user that asked: from sudo (SUDO_UID) or, through the administrator
/// password window, the user logged in at the screen (owner of /dev/console).
fn chamador() -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let uid = match std::env::var("SUDO_UID").ok().and_then(|u| u.parse().ok()) {
        Some(uid) => uid,
        None => std::fs::metadata("/dev/console").ok()?.uid(),
    };
    helper::nome_do_usuario(uid)
}

fn regra_sudo(usuario: &str) -> String {
    format!("{usuario} ALL=(root) NOPASSWD: {HELPER} {ARG_HELPER} *")
}

/// Root side of `autorizar_usuario_atual`: installs the helper, OpenVPN and
/// the DNS script, and the sudo rule for `usuario`. Returns the exit code.
pub fn instalar_para(usuario: &str) -> i32 {
    if unsafe { libc::geteuid() } != 0 {
        return 2;
    }
    // only the user who asked (and that an administrator approved)
    if !nome_valido(usuario) || chamador().as_deref() != Some(usuario) {
        return 3;
    }
    if super::openvpn::EMBUTIDO.is_empty() {
        return 4;
    }
    let Ok(dados) = std::env::current_exe().and_then(std::fs::read) else {
        return 4;
    };
    // the folder must be root's alone (a user-writable folder would let the
    // user swap the files after this check)
    let _ = std::fs::create_dir_all(PASTA);
    if !helper::do_root_e_protegido(PASTA) {
        return 5;
    }
    if helper::gravar_root(HELPER, &dados, 0o755).is_err()
        || helper::gravar_root(OPENVPN, super::openvpn::EMBUTIDO, 0o755).is_err()
        || helper::gravar_root(SCRIPT_DNS, super::openvpn::SCRIPT_DE_DNS, 0o755).is_err()
    {
        return 6;
    }
    // sudo only reads sudoers.d when the main file includes it (it does by default)
    let principal = std::fs::read_to_string("/etc/sudoers").unwrap_or_default();
    if !principal.lines().any(|l| {
        let l = l.trim();
        (l.starts_with("#includedir") || l.starts_with("@includedir")) && l.contains("sudoers.d")
    }) {
        return 9;
    }
    // the sudo rule: the existing users plus this one, checked by visudo
    let _ = std::fs::create_dir_all("/etc/sudoers.d");
    if !helper::do_root_e_protegido("/etc/sudoers.d") {
        return 7;
    }
    let linha = regra_sudo(usuario);
    let mut regras = std::fs::read_to_string(REGRA_SUDO).unwrap_or_default();
    if !regras.lines().any(|l| l == linha) {
        if regras.is_empty() {
            regras.push_str("# VPN app: these users may start OpenVPN through its helper\n");
        }
        regras.push_str(&linha);
        regras.push('\n');
    }
    if helper::gravar_root(REGRA_SUDO_TEMPORARIA, regras.as_bytes(), 0o440).is_err() {
        return 8;
    }
    let valida = Command::new("/usr/sbin/visudo")
        .args(["-c", "-q", "-f", REGRA_SUDO_TEMPORARIA])
        .status()
        .is_ok_and(|s| s.success());
    if !valida || std::fs::rename(REGRA_SUDO_TEMPORARIA, REGRA_SUDO).is_err() {
        let _ = std::fs::remove_file(REGRA_SUDO_TEMPORARIA);
        return 8;
    }
    0
}

/// The helper itself (as root, through sudo). Returns the exit code.
pub fn executar_helper(args: &[String]) -> i32 {
    helper::codigo_de_saida(helper(args))
}

fn helper(args: &[String]) -> Result<i32, String> {
    let uid: u32 = std::env::var("SUDO_UID")
        .ok()
        .and_then(|u| u.parse().ok())
        .ok_or("SUDO_UID")?;
    if !helper::do_root_e_protegido(PASTA) || !helper::do_root_e_protegido(OPENVPN) {
        return Err("the installed OpenVPN is not root-owned".into());
    }
    // the VPN's DNS through OpenVPN's own script (scutil); never the default
    // path compiled into openvpn
    let extras = if helper::do_root_e_protegido(SCRIPT_DNS) {
        vec![
            "--script-security".into(),
            "2".into(),
            "--dns-updown".into(),
            SCRIPT_DNS.into(),
        ]
    } else {
        vec![
            "--script-security".into(),
            "1".into(),
            "--dns-updown".into(),
            "disable".into(),
        ]
    };
    helper::executar_openvpn(
        args,
        Execucao {
            uid,
            openvpn: OPENVPN,
            pasta_privada: Path::new("/var/run/vpn-openvpn-helper"),
            extras,
            path: "/usr/bin:/bin:/usr/sbin:/sbin",
        },
    )
}

/// macOS: OpenVPN creates its utun interface by itself; nothing to create.
pub fn criar_adaptador(_openvpn: &Path, _hwid: &str) -> Result<(), String> {
    Err("tun".into())
}

/// Explanation of the one-time authorization (asked before running it).
pub fn pergunta_de_autorizacao() -> String {
    tr!(
        "Para conectar sem pedir a senha de administrador a cada vez, este \
         usuário do Mac precisa ser autorizado uma única vez (o app instala o \
         OpenVPN e um ajudante em /Library/PrivilegedHelperTools, e uma regra \
         do sudo que vale só para eles).\n\n\
         O Mac vai pedir a senha de um administrador agora. Continuar?",
        "To connect without asking for the administrator password every time, \
         this Mac user needs to be authorized once (the app installs OpenVPN \
         and a helper in /Library/PrivilegedHelperTools, and a sudo rule that \
         applies only to them).\n\n\
         The Mac will ask for an administrator's password now. Continue?"
    )
    .to_string()
}

/// The app run as a helper: the root helper started by sudo, its version
/// check, or the one-time authorization (after the administrator password).
/// Does that job and exits.
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
        std::process::exit(instalar_para(&usuario));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regra_do_sudo_so_para_o_ajudante() {
        assert_eq!(
            regra_sudo("maria"),
            "maria ALL=(root) NOPASSWD: /Library/PrivilegedHelperTools/io.github.antxj.vpn.helper --openvpn-helper *"
        );
        for ruim in ["", "-x", "a b", "maria\nroot ALL=(ALL) ALL", "a,b", "a:b"] {
            assert!(!nome_valido(ruim), "{ruim:?}");
        }
        assert!(nome_valido("maria.silva_2"));
    }

    #[test]
    fn texto_applescript_escapa_aspas() {
        assert_eq!(
            texto_applescript(r#"/Apps/VPN "x"\y.app"#),
            r#""/Apps/VPN \"x\"\\y.app""#
        );
    }
}
