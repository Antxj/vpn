//! Linux: starts OpenVPN as root on behalf of the user, the role the
//! OpenVPN interactive service plays on Windows (the helper itself is in
//! unix/helper.rs).
//!
//! - Once, an administrator authorizes the user (polkit asks for the admin
//!   password): this executable is copied to HELPER (root-owned), the user
//!   joins GROUP, and a polkit rule lets members of GROUP run HELPER with
//!   pkexec without a password.
//! - Each connection runs `pkexec HELPER --openvpn-helper <dir> <options>`,
//!   with the distribution's openvpn and `--script-security 1` (2 with the
//!   distribution's systemd-resolved script, for the VPN's DNS).

use crate::platform::unix::helper::{self, Execucao};
use std::path::Path;
use std::process::Command;

pub use helper::{
    argumento, usuario_atual, Processo, ARG_AUTHORIZE, ARG_HELPER, ARG_VERSAO_DO_AJUDANTE,
};

/// Root-owned copy of this executable that pkexec runs.
pub const HELPER: &str = "/usr/local/libexec/vpn-openvpn-helper";
/// Users authorized to start connections without a password.
const GRUPO: &str = "vpn-openvpn";
const ACAO_POLKIT: &str = "io.github.antxj.vpn.openvpn";
const POLITICA: &str = "/usr/share/polkit-1/actions/io.github.antxj.vpn.policy";
const REGRA_JS: &str = "/etc/polkit-1/rules.d/49-vpn-openvpn.rules";
const REGRA_PKLA: &str = "/etc/polkit-1/localauthority/50-local.d/49-vpn-openvpn.pkla";
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

/// The current user already went through the one-time authorization, and
/// the installed helper is current.
fn autorizado() -> bool {
    Path::new(POLITICA).exists()
        && usuario_no_grupo(&usuario_atual(), GRUPO)
        && helper::versao_confere(HELPER, VERSAO_DO_AJUDANTE)
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
/// with `argumento`).
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
    helper::iniciar(PKEXEC, &[HELPER, ARG_HELPER], workdir, opcoes).map_err(Falha::Outra)
}

/// Windows cleans up openvpn.exe left behind by a crash; on Linux the helper
/// stops openvpn by itself when the app's end of the pipe closes.
pub fn encerrar_se_openvpn(_pid: u32) -> bool {
    false
}

pub fn grupo_autorizado() -> String {
    GRUPO.to_string()
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
    Some((uid, helper::nome_do_usuario(uid)?))
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
    if helper::gravar_root(HELPER, &dados, 0o755).is_err() {
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
    if helper::gravar_root(POLITICA, politica_xml().as_bytes(), 0o644).is_err() {
        return 8;
    }
    // polkit 0.106+ reads JavaScript rules; older versions (Ubuntu 22.04),
    // .pkla files - both are written when their folder exists
    let mut alguma = false;
    if Path::new("/etc/polkit-1/rules.d").is_dir() {
        alguma |= helper::gravar_root(REGRA_JS, regra_js().as_bytes(), 0o644).is_ok();
    }
    if Path::new("/etc/polkit-1/localauthority").is_dir() {
        alguma |= helper::gravar_root(REGRA_PKLA, regra_pkla().as_bytes(), 0o644).is_ok();
    }
    if alguma {
        0
    } else {
        9
    }
}

fn openvpn_do_sistema() -> Option<&'static str> {
    [
        "/usr/sbin/openvpn",
        "/usr/bin/openvpn",
        "/usr/local/sbin/openvpn",
    ]
    .into_iter()
    .find(|p| helper::do_root_e_protegido(p))
}

/// DNS of the VPN: the distribution's script for systemd-resolved, when installed.
fn script_de_dns() -> Option<&'static str> {
    [
        "/etc/openvpn/update-systemd-resolved",
        "/etc/openvpn/scripts/update-systemd-resolved",
    ]
    .into_iter()
    .find(|p| helper::do_root_e_protegido(p))
}

/// The helper itself (as root, through pkexec). Returns the exit code.
pub fn executar_helper(args: &[String]) -> i32 {
    helper::codigo_de_saida(helper(args))
}

fn helper(args: &[String]) -> Result<i32, String> {
    let (uid, _) = chamador().ok_or("PKEXEC_UID")?;
    let openvpn = openvpn_do_sistema().ok_or("openvpn is not installed")?;
    let extras = match script_de_dns() {
        Some(script) => vec![
            "--script-security".into(),
            "2".into(),
            "--up".into(),
            script.into(),
            "--down".into(),
            script.into(),
            "--down-pre".into(),
        ],
        None => vec!["--script-security".into(), "1".into()],
    };
    helper::executar_openvpn(
        args,
        Execucao {
            uid,
            openvpn,
            pasta_privada: Path::new("/run/vpn-openvpn-helper"),
            extras,
            path: "/usr/sbin:/usr/bin:/sbin:/bin",
        },
    )
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

    #[test]
    fn regras_do_polkit() {
        assert!(politica_xml().contains(HELPER));
        assert!(regra_js().contains(GRUPO) && regra_js().contains(ACAO_POLKIT));
        assert!(regra_pkla().contains("Identity=unix-group:vpn-openvpn"));
    }
}
