//! macOS: the icon in the menu bar and notifications. The window itself is
//! egui/winit: showing and hiding it is in unix/window.rs.
//!
//! AppKit only works on the main thread, where eframe runs: the icon is
//! created there (iniciar_bandeja is called while the App is created) and a
//! timer thread hands the updates back to the main thread through the main
//! dispatch queue, also while the window is hidden and egui is idle.

use crate::*;
use std::ffi::c_void;

pub use crate::platform::unix::window::{
    ocultar_antes_do_primeiro_quadro, por_quadro, show_main_window,
};

/// Bundle identifier of VPN.app (Info.plist): notifications show its name and icon.
const IDENTIFICADOR: &str = "io.github.antxj.vpn";

// ------------------------------------------------------------------ tray --

extern "C" {
    /// The main queue (what dispatch_get_main_queue() returns).
    static _dispatch_main_q: c_void;
    fn dispatch_async_f(
        fila: *const c_void,
        contexto: *mut c_void,
        trabalho: extern "C" fn(*mut c_void),
    );
}

/// Runs `f` on the main thread, without waiting.
fn na_thread_principal(f: fn()) {
    extern "C" fn rodar(contexto: *mut c_void) {
        let f: fn() = unsafe { std::mem::transmute::<*mut c_void, fn()>(contexto) };
        f();
    }
    unsafe {
        dispatch_async_f(
            std::ptr::addr_of!(_dispatch_main_q),
            f as *mut c_void,
            rodar,
        );
    }
}

/// Creates the icon in the menu bar and keeps it up to date every second.
pub fn iniciar_bandeja() {
    let _ = notify_rust::set_application(IDENTIFICADOR);
    criar_bandeja();
    apply_tray_state();
    std::thread::spawn(|| loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
        na_thread_principal(apply_tray_state);
    });
}

// --------------------------------------------------------- notifications --

/// Notification of the Notification Center (shown on another thread: the
/// system call may take a moment).
pub fn notificacao(texto: &str, _alerta: bool) -> bool {
    let texto = texto.to_string();
    std::thread::spawn(move || {
        let _ = notify_rust::Notification::new()
            .summary(APP_TITLE)
            .body(&texto)
            .show();
    });
    true
}

/// Notification of a new version (the tray menu has the "Update" item).
pub(crate) fn notificar_nova_versao(numero: &str) {
    if cfg!(test) {
        return;
    }
    notificacao(
        &trf!(
            "Versão {numero} disponível. Atualize pelo ícone na barra de menus.",
            "Version {numero} is available. Update from the icon in the menu bar."
        ),
        false,
    );
}
