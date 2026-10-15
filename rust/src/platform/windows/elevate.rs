//! Administrator rights on demand. The app itself runs as a regular user;
//! only a few occasional tasks need elevation (installing OpenVPN, creating
//! an extra network adapter, authorizing a user for the OpenVPN service).
//! Each one shows the Windows UAC prompt at that moment.

use std::ffi::c_void;
use std::path::Path;

const SEE_MASK_NOCLOSEPROCESS: u32 = 0x0000_0040;
const SEE_MASK_NOASYNC: u32 = 0x0000_0100;
const SW_HIDE: i32 = 0;
const INFINITE: u32 = 0xFFFF_FFFF;
const ERROR_CANCELLED: u32 = 1223;

/// SHELLEXECUTEINFOW (x64 layout).
#[repr(C)]
struct ShellExecuteInfoW {
    cb_size: u32,
    f_mask: u32,
    hwnd: *mut c_void,
    lp_verb: *const u16,
    lp_file: *const u16,
    lp_parameters: *const u16,
    lp_directory: *const u16,
    n_show: i32,
    h_inst_app: *mut c_void,
    lp_id_list: *mut c_void,
    lp_class: *const u16,
    hkey_class: *mut c_void,
    dw_hot_key: u32,
    h_icon_or_monitor: *mut c_void,
    h_process: *mut c_void,
}

#[link(name = "shell32")]
extern "system" {
    fn ShellExecuteExW(info: *mut ShellExecuteInfoW) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn WaitForSingleObject(h: *mut c_void, ms: u32) -> u32;
    fn GetExitCodeProcess(h: *mut c_void, code: *mut u32) -> i32;
    fn CloseHandle(h: *mut c_void) -> i32;
    fn GetLastError() -> u32;
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Runs `exe params` as administrator (UAC prompt), waits for it and returns
/// its exit code. Err with a readable message if the user declines.
pub fn executar_como_admin(exe: &Path, params: &str) -> Result<u32, String> {
    let verbo = wide("runas");
    let arquivo = wide(&exe.to_string_lossy());
    let parametros = wide(params);
    let mut info: ShellExecuteInfoW = unsafe { std::mem::zeroed() };
    info.cb_size = std::mem::size_of::<ShellExecuteInfoW>() as u32;
    info.f_mask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
    info.lp_verb = verbo.as_ptr();
    info.lp_file = arquivo.as_ptr();
    info.lp_parameters = parametros.as_ptr();
    info.n_show = SW_HIDE;

    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        return Err(match unsafe { GetLastError() } {
            ERROR_CANCELLED => tr!(
                "a permissão de administrador foi recusada",
                "the administrator permission was declined"
            )
            .to_string(),
            e => trf!("não foi possível executar como administrador (erro {e})", "could not run as administrator (error {e})"),
        });
    }
    if info.h_process.is_null() {
        return Ok(0);
    }
    let mut codigo = 0u32;
    unsafe {
        WaitForSingleObject(info.h_process, INFINITE);
        GetExitCodeProcess(info.h_process, &mut codigo);
        CloseHandle(info.h_process);
    }
    Ok(codigo)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_do_shellexecuteinfo() {
        // sizeof(SHELLEXECUTEINFOW) on x64 Windows
        assert_eq!(std::mem::size_of::<ShellExecuteInfoW>(), 112);
    }
}
