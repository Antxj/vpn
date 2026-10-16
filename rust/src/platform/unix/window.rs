//! Linux and macOS: showing and hiding the main window (egui/winit
//! viewport commands), and closing it to the tray.

use crate::*;
use std::sync::atomic::Ordering;

/// Shows the main window (from the tray, a notification or a second instance).
pub fn show_main_window() {
    MAIN_WINDOW_VISIBLE.store(true, Ordering::SeqCst);
    if let Some(ctx) = EGUI_CTX.get() {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        ctx.request_repaint();
    }
}

fn esconder(ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
    MAIN_WINDOW_VISIBLE.store(false, Ordering::SeqCst);
}

/// Started minimized: the window is created invisible (see main); nothing
/// to do before the first frame.
pub fn ocultar_antes_do_primeiro_quadro() {}

/// Every frame: closing the window hides it to the tray (the connections
/// stay on), like on Windows.
pub fn por_quadro(ctx: &egui::Context) {
    if INICIAR_OCULTO.swap(false, Ordering::SeqCst) {
        esconder(ctx);
    }
    if ctx.input(|i| i.viewport().close_requested()) {
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        esconder(ctx);
        if !TRAY_HINT_SHOWN.swap(true, Ordering::SeqCst)
            && std::env::var_os("VPN_SKIP_HINT").is_none()
        {
            crate::platform::desktop::notificacao(
                tr!(
                    "O aplicativo VPN continua ativo na bandeja. Clique no ícone para reabrir.",
                    "VPN is still running in the system tray. Click the icon to reopen it."
                ),
                false,
            );
        }
    }
}
