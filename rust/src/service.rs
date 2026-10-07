//! Client of the OpenVPN interactive service (OpenVPNServiceInteractive).
//!
//! The service runs as SYSTEM and starts openvpn.exe on behalf of a regular
//! user, performing the privileged network changes (routes, adapter address,
//! DNS) itself and undoing them when OpenVPN exits. This is what lets the app
//! run without administrator rights - OpenVPN GUI works the same way.
//!
//! Protocol (openvpnserv/interactive.c, OpenVPN 2.4+):
//! - the client writes the UTF-16 string "workdir\0options\0stdin\0" to the
//!   pipe \\.\pipe\openvpn\service;
//! - the service answers "0x00000000\n0x<pid>\nProcess ID" on success, or
//!   "0x<error>\n<function>\n<message>" on failure.
//!
//! Users who are neither administrators nor members of the group configured in
//! the registry (default "OpenVPN Administrators") may only start configs from
//! the global config folder, with a short whitelist of options. The app
//! offers to add such users to that group once (an administrator approves it).

use std::ffi::c_void;
use std::path::Path;
use std::ptr;
use std::time::Duration;

const PIPE: &str = r"\\.\pipe\openvpn\service";
const DEFAULT_ADMIN_GROUP: &str = "OpenVPN Administrators";
/// Command line argument of the elevated helper that authorizes a user.
pub const ARG_AUTHORIZE: &str = "--authorize-openvpn-user";

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const OPEN_EXISTING: u32 = 3;
const PIPE_READMODE_MESSAGE: u32 = 2;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_PIPE_BUSY: u32 = 231;
const ERROR_MORE_DATA: u32 = 234;
const SYNCHRONIZE: u32 = 0x0010_0000;
const PROCESS_TERMINATE: u32 = 0x0001;
const PROCESS_QUERY_INFORMATION: u32 = 0x0400;
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const WAIT_OBJECT_0: u32 = 0;
const INVALID_HANDLE: isize = -1;

#[link(name = "kernel32")]
extern "system" {
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *mut c_void,
        disposition: u32,
        flags: u32,
        template: *mut c_void,
    ) -> *mut c_void;
    fn WaitNamedPipeW(name: *const u16, timeout_ms: u32) -> i32;
    fn SetNamedPipeHandleState(pipe: *mut c_void, mode: *const u32, max: *const u32, timeout: *const u32) -> i32;
    fn WriteFile(h: *mut c_void, buf: *const c_void, len: u32, written: *mut u32, overlapped: *mut c_void) -> i32;
    fn ReadFile(h: *mut c_void, buf: *mut c_void, len: u32, read: *mut u32, overlapped: *mut c_void) -> i32;
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
    fn WaitForSingleObject(h: *mut c_void, ms: u32) -> u32;
    fn GetExitCodeProcess(h: *mut c_void, code: *mut u32) -> i32;
    fn TerminateProcess(h: *mut c_void, code: u32) -> i32;
    fn QueryFullProcessImageNameW(h: *mut c_void, flags: u32, name: *mut u16, size: *mut u32) -> i32;
    fn CloseHandle(h: *mut c_void) -> i32;
    fn GetLastError() -> u32;
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Why OpenVPN could not be started through the service.
#[derive(Debug, PartialEq)]
pub enum Falha {
    /// The service is not installed or not running.
    Indisponivel,
    /// The user needs to be authorized (config outside the global folder or
    /// options outside the whitelist).
    NaoAutorizado(String),
    Outra(String),
}

impl Falha {
    pub fn mensagem(&self) -> String {
        match self {
            Falha::Indisponivel => tr!(
                "O serviço do OpenVPN (OpenVPNServiceInteractive) não está rodando. \
                 Reinstale o OpenVPN Community ou reinicie o computador.",
                "The OpenVPN service (OpenVPNServiceInteractive) is not running. \
                 Reinstall OpenVPN Community or restart the computer."
            )
            .into(),
            Falha::NaoAutorizado(m) | Falha::Outra(m) => m.clone(),
        }
    }
}

/// A process handle closed on Drop.
struct Handle(*mut c_void);
unsafe impl Send for Handle {}
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 as isize != INVALID_HANDLE {
            unsafe { CloseHandle(self.0) };
        }
    }
}

/// openvpn.exe started by the service. The user's token can wait for it,
/// read its exit code and terminate it (the service grants exactly that).
pub struct Processo {
    pub pid: u32,
    processo: Handle,
    _pipe: Handle,
}

impl Processo {
    /// The process could be opened (to follow and stop it).
    pub fn acompanhavel(&self) -> bool {
        !self.processo.0.is_null()
    }

    /// Exit code, if the process has ended.
    pub fn terminou(&self) -> Option<u32> {
        if self.processo.0.is_null() {
            return Some(u32::MAX); // ended before it could be opened
        }
        if unsafe { WaitForSingleObject(self.processo.0, 0) } != WAIT_OBJECT_0 {
            return None;
        }
        let mut code = 0u32;
        unsafe { GetExitCodeProcess(self.processo.0, &mut code) };
        Some(code)
    }

    /// Waits up to `limite` for the process to end; kills it afterwards.
    pub fn esperar(&self, limite: Duration) -> Option<u32> {
        if !self.processo.0.is_null() {
            unsafe { WaitForSingleObject(self.processo.0, limite.as_millis() as u32) };
        }
        if self.terminou().is_none() {
            self.matar();
            std::thread::sleep(Duration::from_millis(500));
        }
        self.terminou()
    }

    pub fn matar(&self) {
        if !self.processo.0.is_null() {
            unsafe { TerminateProcess(self.processo.0, 1) };
        }
    }
}

/// Quotes one argument the way CommandLineToArgvW (used by the service to
/// validate the options) parses it back.
pub fn argumento(a: &str) -> String {
    if !a.is_empty() && !a.contains([' ', '\t', '"']) {
        return a.to_string();
    }
    let mut out = String::from('"');
    let mut barras = 0;
    for c in a.chars() {
        match c {
            '\\' => barras += 1,
            '"' => {
                out.push_str(&"\\".repeat(barras * 2 + 1));
                out.push('"');
                barras = 0;
            }
            _ => {
                out.push_str(&"\\".repeat(barras));
                out.push(c);
                barras = 0;
            }
        }
    }
    out.push_str(&"\\".repeat(barras * 2));
    out.push('"');
    out
}

/// Reads the service answer: Ok(pid) or Err(message).
fn interpretar_resposta(texto: &str) -> Result<u32, (u32, String)> {
    let mut linhas = texto.trim_end_matches('\0').splitn(3, '\n');
    let hex = |s: &str| u32::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok();
    let codigo = linhas.next().and_then(hex).ok_or((u32::MAX, texto.to_string()))?;
    let linha2 = linhas.next().unwrap_or_default().trim().to_string();
    let linha3 = linhas.next().unwrap_or_default().trim().to_string();
    if codigo == 0 {
        return hex(&linha2).ok_or((u32::MAX, texto.to_string()));
    }
    let msg = if linha3.is_empty() { linha2 } else { format!("{linha3} ({linha2})") };
    Err((codigo, msg))
}

/// Starts openvpn.exe through the service, in `workdir`, with `opcoes`
/// (already quoted with `argumento`).
pub fn iniciar(workdir: &Path, opcoes: &str) -> Result<Processo, Falha> {
    let nome = wide(PIPE);
    let mut pipe = unsafe {
        CreateFileW(nome.as_ptr(), GENERIC_READ | GENERIC_WRITE, 0, ptr::null_mut(), OPEN_EXISTING, 0, ptr::null_mut())
    };
    if pipe as isize == INVALID_HANDLE {
        match unsafe { GetLastError() } {
            ERROR_PIPE_BUSY => {
                // another connection is talking to the service right now
                unsafe { WaitNamedPipeW(nome.as_ptr(), 5000) };
                pipe = unsafe {
                    CreateFileW(nome.as_ptr(), GENERIC_READ | GENERIC_WRITE, 0, ptr::null_mut(), OPEN_EXISTING, 0, ptr::null_mut())
                };
            }
            ERROR_FILE_NOT_FOUND => return Err(Falha::Indisponivel),
            e => return Err(Falha::Outra(format!("CreateFile({PIPE}): {e}"))),
        }
        if pipe as isize == INVALID_HANDLE {
            return Err(Falha::Indisponivel);
        }
    }
    let pipe = Handle(pipe);
    let modo = PIPE_READMODE_MESSAGE;
    unsafe { SetNamedPipeHandleState(pipe.0, &modo, ptr::null(), ptr::null()) };

    // workdir \0 options \0 stdin (empty) \0
    let mut dados: Vec<u16> = workdir.to_string_lossy().encode_utf16().collect();
    dados.push(0);
    dados.extend(opcoes.encode_utf16());
    dados.push(0);
    dados.push(0);
    let mut escritos = 0u32;
    let ok = unsafe {
        WriteFile(pipe.0, dados.as_ptr() as *const c_void, (dados.len() * 2) as u32, &mut escritos, ptr::null_mut())
    };
    if ok == 0 {
        return Err(Falha::Outra(format!("WriteFile: {}", unsafe { GetLastError() })));
    }

    let mut buf = [0u16; 2048];
    let mut lidos = 0u32;
    let ok = unsafe {
        ReadFile(pipe.0, buf.as_mut_ptr() as *mut c_void, (buf.len() * 2) as u32, &mut lidos, ptr::null_mut())
    };
    if ok == 0 && unsafe { GetLastError() } != ERROR_MORE_DATA {
        return Err(Falha::Outra(format!("ReadFile: {}", unsafe { GetLastError() })));
    }
    let resposta = String::from_utf16_lossy(&buf[..(lidos as usize / 2)]);
    match interpretar_resposta(&resposta) {
        Ok(pid) => {
            // exactly the rights the service grants the user on that process
            let acesso = SYNCHRONIZE | PROCESS_TERMINATE | PROCESS_QUERY_INFORMATION;
            let processo = Handle(unsafe { OpenProcess(acesso, 0, pid) });
            Ok(Processo { pid, processo, _pipe: pipe })
        }
        Err((_, msg)) if msg.contains("admin approval") => Err(Falha::NaoAutorizado(msg)),
        Err((codigo, msg)) => Err(Falha::Outra(format!("0x{codigo:08x}: {msg}"))),
    }
}

/// Terminates an openvpn.exe left behind by a previous run of the app (e.g.
/// killed by the Task Manager): without the app, nobody would answer its
/// password prompts. Checks the image name, since PIDs are reused.
pub fn encerrar_se_openvpn(pid: u32) -> bool {
    let h = Handle(unsafe { OpenProcess(PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) });
    if h.0.is_null() {
        return false;
    }
    let mut nome = [0u16; 1024];
    let mut tamanho = nome.len() as u32;
    if unsafe { QueryFullProcessImageNameW(h.0, 0, nome.as_mut_ptr(), &mut tamanho) } == 0 {
        return false;
    }
    let caminho = String::from_utf16_lossy(&nome[..tamanho as usize]).to_lowercase();
    if !caminho.ends_with("\\openvpn.exe") {
        return false;
    }
    unsafe { TerminateProcess(h.0, 1) != 0 }
}

// ------------------------------------------------- user authorization --

/// Group whose members may use any config/option (registry ovpn_admin_group).
pub fn grupo_autorizado() -> String {
    use windows_sys::Win32::System::Registry::{
        RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6464KEY,
    };
    let chave = wide(r"SOFTWARE\OpenVPN");
    let valor = wide("ovpn_admin_group");
    let mut buf = [0u16; 256];
    let mut len = (buf.len() * 2) as u32;
    let ok = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            chave.as_ptr(),
            valor.as_ptr(),
            RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
            ptr::null_mut(),
            buf.as_mut_ptr() as *mut c_void,
            &mut len,
        )
    };
    if ok == 0 {
        let n = (len as usize / 2).saturating_sub(1).min(buf.len());
        let nome = String::from_utf16_lossy(&buf[..n]);
        if !nome.trim().is_empty() {
            return nome;
        }
    }
    DEFAULT_ADMIN_GROUP.to_string()
}

/// Current Windows user as DOMAIN\name (or COMPUTER\name for local accounts).
pub fn usuario_atual() -> String {
    let dominio = std::env::var("USERDOMAIN").unwrap_or_default();
    let nome = std::env::var("USERNAME").unwrap_or_default();
    if dominio.is_empty() {
        nome
    } else {
        format!("{dominio}\\{nome}")
    }
}

/// Asks Windows for administrator approval (UAC) to add the current user to
/// the authorized group. Runs this same executable elevated with ARG_AUTHORIZE.
pub fn autorizar_usuario_atual() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let params = format!("{ARG_AUTHORIZE} {}", argumento(&usuario_atual()));
    match crate::elevate::executar_como_admin(&exe, &params)? {
        0 => Ok(()),
        codigo => Err(trf!(
            "não foi possível autorizar o usuário (código {codigo})",
            "could not authorize the user (code {codigo})"
        )),
    }
}

/// Elevated side of `autorizar_usuario_atual`: creates the group if needed
/// and adds the user to it. Returns the process exit code.
pub fn adicionar_ao_grupo(usuario: &str) -> i32 {
    #[repr(C)]
    struct LocalGroupInfo1 {
        name: *const u16,
        comment: *const u16,
    }
    #[repr(C)]
    struct LocalGroupMembersInfo3 {
        domain_and_name: *const u16,
    }
    #[link(name = "netapi32")]
    extern "system" {
        fn NetLocalGroupAdd(server: *const u16, level: u32, buf: *const u8, parm_err: *mut u32) -> u32;
        fn NetLocalGroupAddMembers(server: *const u16, group: *const u16, level: u32, buf: *const u8, total: u32) -> u32;
    }
    const NERR_SUCCESS: u32 = 0;
    const NERR_GROUP_EXISTS: u32 = 2223;
    const ERROR_ALIAS_EXISTS: u32 = 1379;
    const ERROR_MEMBER_IN_ALIAS: u32 = 1378;

    let grupo = wide(&grupo_autorizado());
    let comentario = wide("Users allowed to run any OpenVPN configuration");
    let info = LocalGroupInfo1 { name: grupo.as_ptr(), comment: comentario.as_ptr() };
    let mut erro_parm = 0u32;
    let r = unsafe { NetLocalGroupAdd(ptr::null(), 1, &info as *const _ as *const u8, &mut erro_parm) };
    if !matches!(r, NERR_SUCCESS | NERR_GROUP_EXISTS | ERROR_ALIAS_EXISTS) {
        return r as i32;
    }
    let nome = wide(usuario);
    let membro = LocalGroupMembersInfo3 { domain_and_name: nome.as_ptr() };
    let r = unsafe { NetLocalGroupAddMembers(ptr::null(), grupo.as_ptr(), 3, &membro as *const _ as *const u8, 1) };
    match r {
        NERR_SUCCESS | ERROR_MEMBER_IN_ALIAS => 0,
        e => e as i32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Splits with Windows' own CommandLineToArgvW - the same parser the
    /// service uses to validate the options.
    fn dividir(linha: &str) -> Vec<String> {
        use windows_sys::Win32::UI::Shell::CommandLineToArgvW;
        #[link(name = "kernel32")]
        extern "system" {
            fn LocalFree(h: *mut c_void) -> *mut c_void;
        }
        let w = wide(linha);
        let mut n = 0i32;
        let argv = unsafe { CommandLineToArgvW(w.as_ptr(), &mut n) };
        let mut out = Vec::new();
        for i in 0..n as usize {
            unsafe {
                let p = *argv.add(i);
                let mut len = 0;
                while *p.add(len) != 0 {
                    len += 1;
                }
                out.push(String::from_utf16_lossy(std::slice::from_raw_parts(p, len)));
            }
        }
        unsafe { LocalFree(argv as *mut c_void) };
        out
    }

    #[test]
    fn argumentos_sobrevivem_ao_parser_do_windows() {
        let casos = [
            r"C:\Users\Maria Silva\OpenVPN\config\trabalho.ovpn",
            r"C:\semespaco\a.ovpn",
            r#"tem "aspas" no meio"#,
            r"termina com barra\",
            r"\\servidor\pasta compartilhada\x.ovpn",
            "",
        ];
        // first word: CommandLineToArgvW treats argv[0] as the program name
        let linha = std::iter::once("openvpn".to_string())
            .chain(casos.iter().map(|c| argumento(c)))
            .collect::<Vec<_>>()
            .join(" ");
        let volta = dividir(&linha);
        assert_eq!(&volta[1..], &casos.map(String::from)[..]);
    }

    #[test]
    fn le_a_resposta_do_servico() {
        assert_eq!(interpretar_resposta("0x00000000\n0x00001a2b\nProcess ID"), Ok(0x1a2b));
        let erro = interpretar_resposta(
            "0x20000000\nYou have specified an option (--auth-nocache) that may be used only with \
             admin approval.\nGetStartupData",
        );
        let (codigo, msg) = erro.unwrap_err();
        assert_eq!(codigo, 0x2000_0000);
        assert!(msg.contains("admin approval"));
        assert!(interpretar_resposta("lixo").is_err());
    }

    /// Starts a real openvpn.exe through the service, with a config that
    /// fails right away (no certificate): checks the whole protocol.
    /// Needs OpenVPN 2.4+ installed: cargo test -- --ignored servico_real
    #[test]
    #[ignore]
    fn servico_real_inicia_o_openvpn() {
        let dir = std::env::temp_dir().join(format!("vpn-teste-servico-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ovpn = dir.join("teste.ovpn");
        std::fs::write(&ovpn, "client\ndev tun\nremote 203.0.113.1 1194\n").unwrap();
        let opcoes = format!("--config {}", argumento(&ovpn.to_string_lossy()));
        let p = iniciar(&dir, &opcoes).expect("the service should start openvpn");
        assert!(p.pid > 0);
        assert!(p.acompanhavel(), "could not open the openvpn process");
        // no certificate: openvpn exits by itself with an options error
        let codigo = p.esperar(Duration::from_secs(15));
        assert!(codigo.is_some_and(|c| c != 0), "exit code: {codigo:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
