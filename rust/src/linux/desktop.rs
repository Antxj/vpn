//! Linux: main window, tray icon, notifications and native dialogs.
//!
//! GTK only works from the thread that started it, so a single thread of
//! our own runs GTK for everything that needs it: the tray icon
//! (AppIndicator, through tray-icon) and the dialogs (src/dialogo.rs).
//! The window itself is egui/winit (X11 or Wayland) and needs no GTK.

use crate::*;
use std::sync::mpsc;
use std::sync::OnceLock;
use std::thread::ThreadId;

static THREAD_GTK: OnceLock<ThreadId> = OnceLock::new();
static GTK_OK: OnceLock<bool> = OnceLock::new();

/// Starts the GTK thread on the first use. False when GTK cannot start
/// (no graphical session).
fn garantir_gtk() -> bool {
    *GTK_OK.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        let criada = std::thread::Builder::new().name("gtk".into()).spawn(move || {
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
fn na_thread_gtk<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
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

// ---------------------------------------------------------------- window --

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
    if std::env::var_os("VPN_DEBUG_QUADROS").is_some() {
        eprintln!("QUADRO {} visivel={}", ctx.cumulative_pass_nr(), MAIN_WINDOW_VISIBLE.load(Ordering::SeqCst));
    }
    if INICIAR_OCULTO.swap(false, Ordering::SeqCst) {
        esconder(ctx);
    }
    if ctx.input(|i| i.viewport().close_requested()) {
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        esconder(ctx);
        if !TRAY_HINT_SHOWN.swap(true, Ordering::SeqCst) && std::env::var_os("VPN_SKIP_HINT").is_none() {
            notificacao(
                tr!(
                    "O aplicativo VPN continua ativo na bandeja. Clique no ícone para reabrir.",
                    "VPN is still running in the system tray. Click the icon to reopen it."
                ),
                false,
            );
        }
    }
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
        .icon(if alerta { "network-vpn-disconnected" } else { "network-vpn" })
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

// --------------------------------------------------------------- dialogs --

pub mod dialogos {
    use crate::dialogo::{Filtro, Nivel};
    use gtk::prelude::*;
    use std::path::PathBuf;

    fn caixa(texto: &str, nivel: Nivel, pergunta: bool) -> bool {
        let tipo = match nivel {
            Nivel::Info => gtk::MessageType::Info,
            Nivel::Aviso => gtk::MessageType::Warning,
            Nivel::Erro => gtk::MessageType::Error,
        };
        let botoes = if pergunta { gtk::ButtonsType::YesNo } else { gtk::ButtonsType::Ok };
        let d = gtk::MessageDialog::new(
            None::<&gtk::Window>,
            gtk::DialogFlags::MODAL,
            tipo,
            botoes,
            texto,
        );
        d.set_title(crate::APP_TITLE);
        d.set_keep_above(true);
        d.set_position(gtk::WindowPosition::Center);
        let resposta = d.run();
        unsafe { d.destroy() };
        while gtk::events_pending() {
            gtk::main_iteration();
        }
        resposta == gtk::ResponseType::Yes
    }

    pub fn mensagem(texto: &str, nivel: Nivel) {
        let texto = texto.to_string();
        super::na_thread_gtk(move || {
            caixa(&texto, nivel, false);
        });
    }

    pub fn sim_nao(texto: &str, nivel: Nivel) -> bool {
        let texto = texto.to_string();
        super::na_thread_gtk(move || caixa(&texto, nivel, true)).unwrap_or(false)
    }

    /// `salvar`: Some(suggested name) for a save dialog.
    fn escolher(
        titulo: &str,
        filtro_nome: &str,
        extensoes: &[String],
        pasta: Option<PathBuf>,
        salvar: Option<String>,
    ) -> Option<PathBuf> {
        let (acao, botao) = if salvar.is_some() {
            (gtk::FileChooserAction::Save, tr!("_Salvar", "_Save"))
        } else {
            (gtk::FileChooserAction::Open, tr!("_Abrir", "_Open"))
        };
        let d = gtk::FileChooserDialog::with_buttons(
            Some(titulo),
            None::<&gtk::Window>,
            acao,
            &[
                (tr!("_Cancelar", "_Cancel"), gtk::ResponseType::Cancel),
                (botao, gtk::ResponseType::Accept),
            ],
        );
        let filtro = gtk::FileFilter::new();
        filtro.set_name(Some(filtro_nome));
        for e in extensoes {
            filtro.add_pattern(&format!("*.{e}"));
            filtro.add_pattern(&format!("*.{}", e.to_uppercase()));
        }
        d.add_filter(filtro);
        if let Some(p) = pasta {
            let _ = d.set_current_folder(p);
        }
        if let Some(nome) = &salvar {
            d.set_do_overwrite_confirmation(true);
            d.set_current_name(nome);
        }
        d.set_keep_above(true);
        let resposta = d.run();
        let arquivo = d.filename();
        unsafe { d.destroy() };
        while gtk::events_pending() {
            gtk::main_iteration();
        }
        if resposta != gtk::ResponseType::Accept {
            return None;
        }
        let mut arquivo = arquivo?;
        if salvar.is_some() && arquivo.extension().is_none() {
            if let Some(e) = extensoes.first() {
                arquivo.set_extension(e);
            }
        }
        Some(arquivo)
    }

    pub fn abrir_arquivo(titulo: &str, filtro: Filtro, pasta: Option<PathBuf>) -> Option<PathBuf> {
        let (titulo, nome) = (titulo.to_string(), filtro.nome.to_string());
        let extensoes: Vec<String> = filtro.extensoes.iter().map(|e| e.to_string()).collect();
        super::na_thread_gtk(move || escolher(&titulo, &nome, &extensoes, pasta, None)).flatten()
    }

    pub fn salvar_arquivo(titulo: &str, filtro: Filtro, sugerido: &str) -> Option<PathBuf> {
        let (titulo, nome, sugerido) = (titulo.to_string(), filtro.nome.to_string(), sugerido.to_string());
        let extensoes: Vec<String> = filtro.extensoes.iter().map(|e| e.to_string()).collect();
        super::na_thread_gtk(move || escolher(&titulo, &nome, &extensoes, None, Some(sugerido))).flatten()
    }
}
