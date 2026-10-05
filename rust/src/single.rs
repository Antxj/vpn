//! Instancia unica: mutex nomeado + evento nomeado para "mostre-se".

use std::ffi::c_void;
use std::ptr;

const ERROR_ALREADY_EXISTS: u32 = 183;
const EVENT_MODIFY_STATE: u32 = 0x0002;
const WAIT_OBJECT_0: u32 = 0;

#[link(name = "kernel32")]
extern "system" {
    fn CreateMutexW(attrs: *mut c_void, initial_owner: i32, name: *const u16) -> *mut c_void;
    fn CreateEventW(
        attrs: *mut c_void,
        manual_reset: i32,
        initial_state: i32,
        name: *const u16,
    ) -> *mut c_void;
    fn OpenEventW(access: u32, inherit: i32, name: *const u16) -> *mut c_void;
    fn SetEvent(handle: *mut c_void) -> i32;
    fn WaitForSingleObject(handle: *mut c_void, ms: u32) -> u32;
    fn GetLastError() -> u32;
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// Duas instancias disputariam adaptadores e rotas: a segunda so pede para
// a primeira aparecer e sai.
const MUTEX_NAME: &str = "Local\\vpn-rs-instancia-unica";
const EVENT_NAME: &str = "Local\\vpn-rs-mostrar";

/// Nome do objeto do Windows. VPN_INSTANCIA (so para testes) separa uma
/// instancia de teste do app de uso diario aberto na mesma sessao.
fn nome(base: &str) -> Vec<u16> {
    match std::env::var("VPN_INSTANCIA") {
        Ok(sufixo) if !sufixo.is_empty() => wide(&format!("{base}-{sufixo}")),
        _ => wide(base),
    }
}

/// Retorna true se esta e a primeira instancia (e mantem o mutex vivo pelo
/// resto do processo). Se ja houver outra, sinaliza-a e retorna false.
pub fn acquire_or_signal() -> bool {
    let name = nome(MUTEX_NAME);
    let handle = unsafe { CreateMutexW(ptr::null_mut(), 0, name.as_ptr()) };
    let already = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    // handle intencionalmente "vazado": vive ate o fim do processo
    let _ = handle;
    if !already {
        return true;
    }
    let ev_name = nome(EVENT_NAME);
    let ev = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, ev_name.as_ptr()) };
    if !ev.is_null() {
        unsafe { SetEvent(ev) };
    }
    false
}

/// Cria o evento "mostre-se" e uma thread que restaura a janela principal
/// (direto pela API do Windows - funciona mesmo com a UI oculta/pausada)
/// sempre que uma segunda instancia sinalizar.
pub fn spawn_show_listener() {
    let ev_name = nome(EVENT_NAME);
    let ev = unsafe { CreateEventW(ptr::null_mut(), 0, 0, ev_name.as_ptr()) } as usize;
    if ev == 0 {
        return;
    }
    std::thread::spawn(move || loop {
        let r = unsafe { WaitForSingleObject(ev as *mut c_void, 60_000) };
        if r == WAIT_OBJECT_0 {
            crate::show_main_window();
        }
    });
}
