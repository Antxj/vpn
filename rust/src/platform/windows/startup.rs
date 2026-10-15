//! "Start with Windows": a value in the user's Run key
//! (HKCU\Software\Microsoft\Windows\CurrentVersion\Run) pointing to this
//! executable with ARG_STARTUP. No administrator rights needed - since 1.1.0
//! the app runs as a regular user, so Windows starts it normally at logon.

use std::ffi::c_void;
use std::ptr;
use windows_sys::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
};

const CHAVE: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// Command line argument that marks a start made by Windows at logon.
pub const ARG_STARTUP: &str = "--startup";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Name of the value. A test instance (VPN_INSTANCE) uses its own name, so
/// tests never touch the everyday app's entry.
fn nome_do_valor() -> String {
    match std::env::var("VPN_INSTANCE") {
        Ok(s) if !s.is_empty() => format!("VPN-{s}"),
        _ => "VPN".into(),
    }
}

/// Command written in the registry: this executable + ARG_STARTUP.
fn comando() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    Some(format!("\"{}\" {ARG_STARTUP}", exe.display()))
}

fn ler() -> Option<String> {
    ler_valor(&nome_do_valor())
}

fn ler_valor(nome: &str) -> Option<String> {
    let chave = wide(CHAVE);
    let valor = wide(nome);
    let mut buf = [0u16; 1024];
    let mut len = (buf.len() * 2) as u32;
    let ok = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            chave.as_ptr(),
            valor.as_ptr(),
            RRF_RT_REG_SZ,
            ptr::null_mut(),
            buf.as_mut_ptr() as *mut c_void,
            &mut len,
        )
    };
    if ok != 0 {
        return None;
    }
    let n = (len as usize / 2).saturating_sub(1).min(buf.len());
    Some(String::from_utf16_lossy(&buf[..n]))
}

/// The app starts with Windows.
pub fn ativo() -> bool {
    ler().is_some()
}

/// Turns "start with Windows" on or off.
pub fn definir(ligar: bool) -> Result<(), String> {
    definir_valor(&nome_do_valor(), ligar)
}

fn definir_valor(nome: &str, ligar: bool) -> Result<(), String> {
    let chave = wide(CHAVE);
    let valor = wide(nome);
    let r = if ligar {
        let cmd = wide(&comando().ok_or_else(|| "current_exe".to_string())?);
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                chave.as_ptr(),
                valor.as_ptr(),
                REG_SZ,
                cmd.as_ptr() as *const c_void,
                (cmd.len() * 2) as u32,
            )
        }
    } else {
        const ERROR_FILE_NOT_FOUND: u32 = 2;
        match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, chave.as_ptr(), valor.as_ptr()) } {
            ERROR_FILE_NOT_FOUND => 0,
            r => r,
        }
    };
    if r == 0 {
        Ok(())
    } else {
        Err(trf!("erro {r} no registro do Windows", "Windows registry error {r}"))
    }
}

/// If the executable was moved since the option was turned on, points the
/// entry to the current location (otherwise Windows would start nothing).
pub fn corrigir_caminho() {
    if let (Some(atual), Some(certo)) = (ler(), comando()) {
        if !atual.eq_ignore_ascii_case(&certo) {
            let _ = definir(true);
        }
    }
}

/// This process was started by Windows at logon.
pub fn iniciado_pelo_windows() -> bool {
    std::env::args().any(|a| a == ARG_STARTUP)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Turns the option on and off under a test-only value name (never the
    /// everyday app's entry).
    #[test]
    fn liga_e_desliga_no_registro() {
        let nome = format!("VPN-teste-startup-{}", std::process::id());
        assert!(ler_valor(&nome).is_none());
        definir_valor(&nome, true).unwrap();
        let cmd = ler_valor(&nome).unwrap();
        assert!(cmd.ends_with(ARG_STARTUP), "{cmd}");
        assert!(cmd.starts_with('"'), "{cmd}");
        definir_valor(&nome, false).unwrap();
        assert!(ler_valor(&nome).is_none());
        // turning off twice is not an error
        definir_valor(&nome, false).unwrap();
    }
}
