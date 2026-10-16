//! Linux: tray icon and notifications. The window itself is egui/winit (X11
//! or Wayland) and needs no GTK: showing and hiding it is in unix/window.rs.
//!
//! GTK only works from the thread that started it, so a single thread of
//! our own runs GTK for everything that needs it: the tray icon
//! (AppIndicator, through tray-icon) and the dialogs (dialogs.rs).

use crate::*;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::OnceLock;
use std::thread::ThreadId;

pub use crate::platform::unix::window::{
    ocultar_antes_do_primeiro_quadro, por_quadro, show_main_window,
};

static THREAD_GTK: OnceLock<ThreadId> = OnceLock::new();
static GTK_OK: OnceLock<bool> = OnceLock::new();

/// Starts the GTK thread on the first use. False when GTK cannot start
/// (no graphical session).
fn garantir_gtk() -> bool {
    *GTK_OK.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        let criada = std::thread::Builder::new()
            .name("gtk".into())
            .spawn(move || {
                let ok = gtk::init().is_ok();
                let _ = THREAD_GTK.set(std::thread::current().id());
                let _ = tx.send(ok);
                if ok {
                    gtk::main();
                }
            });
        criada.is_ok() && rx.recv().unwrap_or(false)
    })
}

/// Runs `f` on the GTK thread and waits for the result (directly, when
/// already on it: e.g. a dialog opened from the tray menu).
pub(super) fn na_thread_gtk<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    if !garantir_gtk() {
        return None;
    }
    if THREAD_GTK.get() == Some(&std::thread::current().id()) {
        return Some(f());
    }
    let (tx, rx) = mpsc::channel();
    gtk::glib::MainContext::default().invoke(move || {
        let _ = tx.send(f());
    });
    rx.recv().ok()
}

// ------------------------------------------------------------------ tray --

/// Creates the tray icon on the GTK thread and keeps it up to date every
/// second (also while the window is hidden and egui is idle).
pub fn iniciar_bandeja() {
    na_thread_gtk(|| {
        criar_bandeja();
        apply_tray_state();
        gtk::glib::timeout_add_local(std::time::Duration::from_secs(1), || {
            apply_tray_state();
            gtk::glib::ControlFlow::Continue
        });
    });
}

// --------------------------------------------------------- notifications --

/// Desktop notification (freedesktop). Clicking it opens the app.
pub fn notificacao(texto: &str, alerta: bool) -> bool {
    notificar(texto, alerta, false)
}

/// Notification of a new version: clicking it opens the update window.
pub(crate) fn notificar_nova_versao(numero: &str) {
    if cfg!(test) {
        return;
    }
    let texto = trf!(
        "Versão {numero} disponível. Clique aqui para atualizar.",
        "Version {numero} is available. Click here to update."
    );
    notificar(&texto, false, true);
}

fn notificar(texto: &str, alerta: bool, versao: bool) -> bool {
    let mut n = notify_rust::Notification::new();
    n.summary(APP_TITLE)
        .body(texto)
        .appname(APP_TITLE)
        .icon(if alerta {
            "network-vpn-disconnected"
        } else {
            "network-vpn"
        })
        .action("default", tr!("Abrir", "Open"))
        .timeout(notify_rust::Timeout::Milliseconds(6000));
    match n.show() {
        Ok(aviso) => {
            std::thread::spawn(move || {
                aviso.wait_for_action(|acao| {
                    if acao == "default" {
                        if versao {
                            ABRIR_ATUALIZACAO.store(true, Ordering::SeqCst);
                        }
                        show_main_window();
                    }
                });
            });
            true
        }
        Err(_) => false,
    }
}
