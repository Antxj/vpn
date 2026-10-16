//! VPN - OpenVPN connections (one or several at the same time) with a TOTP
//! token generated automatically.
//!
//! This file starts the app and keeps the state shared by the whole app; the
//! window is in ui/, the tray icon in tray.rs and what talks to the operating
//! system in platform/<os>/.
#![windows_subsystem = "windows"]

// first: the tr!/trf! macros must be defined before the modules
#[macro_use]
mod i18n;
mod accounts;
mod backup;
mod dialogs;
mod engine;
mod qr;
mod routes;
mod settings;
mod state;
mod support;
mod totp;
mod update;
mod vpn;

// what talks to the operating system: platform/windows or platform/linux
mod platform;
mod tray;
mod ui;

pub(crate) use desktop::show_main_window;
use platform::{desktop, installer, renderer, service, single, startup};

use eframe::egui;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tray::*;
use ui::App;

const APP_TITLE: &str = "VPN";
const WINDOW_TITLE: &str = concat!("VPN v", env!("CARGO_PKG_VERSION"));

/// Window size (fixed: the window is not resizable).
const WINDOW_SIZE: [f32; 2] = [480.0, 660.0];
const OPENVPN_CHECK_INTERVAL: Duration = Duration::from_secs(5);

static MAIN_WINDOW_VISIBLE: AtomicBool = AtomicBool::new(true);
static TRAY_HINT_SHOWN: AtomicBool = AtomicBool::new(false);
/// Request from the tray menu to open the update window.
static ABRIR_ATUALIZACAO: AtomicBool = AtomicBool::new(false);
/// Started by Windows with "start minimized": hide the window on the first frame.
static INICIAR_OCULTO: AtomicBool = AtomicBool::new(false);
/// This start came from an update done by the app itself.
static APOS_ATUALIZAR: AtomicBool = AtomicBool::new(false);
/// Accounts that were on before the update (turned back on at start).
static RECONECTAR: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
/// Global egui context for the threads (filled in when the App is created).
static EGUI_CTX: std::sync::OnceLock<egui::Context> = std::sync::OnceLock::new();
fn eframe_ctx() -> egui::Context {
    EGUI_CTX.get().cloned().unwrap_or_default()
}

fn find_openvpn() -> Option<PathBuf> {
    // VPN_OPENVPN points to an openvpn.exe outside the default path (and,
    // with a non-existent path, tests the missing-OpenVPN notice)
    if let Ok(custom) = std::env::var("VPN_OPENVPN") {
        let p = PathBuf::from(custom);
        return if p.exists() { Some(p) } else { None };
    }
    platform::openvpn::encontrar()
}

fn find_default_config() -> Option<PathBuf> {
    for dir in platform::openvpn::pastas_de_config() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut ovpns: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("ovpn"))
            })
            .collect();
        ovpns.sort();
        if let Some(first) = ovpns.into_iter().next() {
            return Some(first);
        }
    }
    None
}

fn load_icon_rgba(bytes: &[u8]) -> (Vec<u8>, u32, u32) {
    let img = image::load_from_memory(bytes).expect("invalid embedded icon");
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    (rgba.into_raw(), w, h)
}

fn error_box(msg: &str) {
    dialogs::mensagem(msg, dialogs::Nivel::Erro);
}

fn report_startup_error(error: &eframe::Error) {
    let log_info = registrar_erro_de_inicio(error);
    error_box(&trf!(
        "Não foi possível iniciar a interface gráfica.\n\nErro: {error}\n\n{log_info}",
        "Could not start the user interface.\n\nError: {error}\n\n{log_info}"
    ));
}

/// Appends the error to startup-error.log; returns where (or why not), for the user.
fn registrar_erro_de_inicio(error: &eframe::Error) -> String {
    let app_dir = settings::app_dir();
    let log_path = app_dir.join("startup-error.log");
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let entry =
        format!("[unix {timestamp}] {WINDOW_TITLE}\nErro: {error}\nDetalhes: {error:#?}\n\n");

    let log_result = std::fs::create_dir_all(&app_dir).and_then(|_| {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?
            .write_all(entry.as_bytes())
    });

    match log_result {
        Ok(()) => trf!(
            "Detalhes registrados em:\n{}",
            "Details logged in:\n{}",
            log_path.display()
        ),
        Err(log_error) => trf!(
            "Não foi possível gravar o log: {log_error}",
            "Could not write the log: {log_error}"
        ),
    }
}

/// "1234567" bytes -> "1,2 MB" (pt-BR decimal comma).
fn fmt_bytes(bytes: f64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes.max(0.0);
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", v as u64, UNITS[unit])
    } else {
        let texto = format!("{v:.1} {}", UNITS[unit]);
        // decimal comma only in Portuguese
        if i18n::pt() {
            texto.replace('.', ",")
        } else {
            texto
        }
    }
}

fn fmt_rate(bytes_per_sec: f64) -> String {
    format!("{}/s", fmt_bytes(bytes_per_sec))
}

/// Windows notification about a connection (drop, back again), when the
/// user wants them. Callable from any thread.
pub fn notificar(texto: String, alerta: bool) {
    if cfg!(test) {
        state::log("", texto); // no notifications during tests
        return;
    }
    if !engine::get().avisos_de_queda() {
        return;
    }
    desktop::notificacao(&texto, alerta);
}

/// Shows an error without blocking the caller (connection threads).
pub fn error_box_async(msg: String) {
    if cfg!(test) {
        state::log("", msg); // no modal windows during tests
        return;
    }
    std::thread::spawn(move || error_box(&msg));
}

fn main() -> eframe::Result<()> {
    // the app run as a helper (user authorization, Linux root helper):
    // does that job and exits
    service::tratar_argumentos_especiais();

    // after "Update now", the previous version opens this one passing its own
    // PID and the accounts that were on
    let args: Vec<String> = std::env::args().collect();
    let valor = |nome: &str| {
        args.iter()
            .position(|a| a == nome)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    if let Some(pid) = valor(update::ARG_APOS_ATUALIZAR).and_then(|p| p.parse().ok()) {
        // the previous one is still disconnecting the VPNs (up to 15 s) and holds the instance
        update::aguardar_processo(pid, Duration::from_secs(30));
        APOS_ATUALIZAR.store(true, Ordering::SeqCst);
    }
    // OpenGL could not open the window: the previous process opened this one
    // with DirectX 12 and is exiting (it holds the single instance until then)
    let forcar_dx12 = match valor(renderer::ARG_DX12) {
        Some(pid) => {
            if let Ok(pid) = pid.parse() {
                update::aguardar_processo(pid, Duration::from_secs(15));
            }
            true
        }
        None => false,
    };
    if let Some(ids) = valor(update::ARG_RECONECTAR) {
        let _ = RECONECTAR.set(
            ids.split(',')
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect(),
        );
    }

    if !single::acquire_or_signal() {
        return Ok(()); // another instance is already running and was notified
    }
    update::limpar_restos();
    // openvpn.exe left behind by a previous run that did not end normally
    let orfaos = vpn::encerrar_orfaos();
    if orfaos > 0 {
        state::log(
            "",
            trf!(
                "{orfaos} conexão(ões) deixada(s) por uma execução anterior foi(ram) encerrada(s).",
                "{orfaos} connection(s) left behind by a previous run were closed."
            ),
        );
    }
    i18n::aplicar(engine::get().idioma());
    startup::corrigir_caminho();
    // started by Windows at logon with "start minimized": tray icon only
    let oculto = startup::iniciado_pelo_windows() && engine::get().iniciar_minimizado();
    if oculto {
        INICIAR_OCULTO.store(true, Ordering::SeqCst);
    }

    let (rgba, w, h) = load_icon_rgba(include_bytes!("../assets/app_64.png"));
    let icon = egui::IconData {
        rgba,
        width: w,
        height: h,
    };

    let (desenho, motivo) = renderer::escolher(forcar_dx12);
    // only the exception goes to the log (useful when the log is sent to support)
    if desenho.excecao() {
        state::log(
            "",
            trf!(
                "Interface: {} ({motivo})",
                "Interface: {} ({motivo})",
                desenho.nome()
            ),
        );
    }

    let viewport = egui::ViewportBuilder::default()
        .with_inner_size(WINDOW_SIZE)
        .with_resizable(false)
        .with_icon(std::sync::Arc::new(icon))
        .with_title(WINDOW_TITLE)
        .with_app_id("vpn");
    let options = renderer::opcoes(desenho, viewport, oculto);

    if desenho == renderer::Desenho::OpenGl {
        renderer::vigiar_inicio_opengl();
    }
    let result = eframe::run_native(
        WINDOW_TITLE,
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    );

    if let Err(error) = &result {
        // OpenGL could not open the window: the app opens again with DirectX 12
        if renderer::falhou_ao_abrir() {
            registrar_erro_de_inicio(error);
            return Ok(());
        }
        report_startup_error(error);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Situacao;

    #[test]
    fn formatacao_de_bytes() {
        assert_eq!(fmt_bytes(0.0), "0 B");
        assert_eq!(fmt_bytes(512.0), "512 B");
        assert_eq!(fmt_bytes(1536.0), "1,5 KB");
        assert_eq!(fmt_bytes(1024.0 * 1024.0 * 2.3), "2,3 MB");
        assert_eq!(fmt_rate(1024.0 * 120.0), "120,0 KB/s");
    }

    #[test]
    fn mapeamento_de_estado_do_openvpn() {
        assert_eq!(state::situacao_do_openvpn("CONNECTED"), Situacao::Conectado);
        assert_eq!(
            state::situacao_do_openvpn("RECONNECTING"),
            Situacao::Reconectando
        );
        assert_eq!(
            state::situacao_do_openvpn("EXITING"),
            Situacao::Desconectando
        );
        assert_eq!(state::situacao_do_openvpn("WAIT"), Situacao::Conectando);
    }

    /// A string broken over several source lines without the trailing
    /// backslash keeps the indentation inside the text (a long run of spaces
    /// in the middle of a sentence).
    #[test]
    fn textos_sem_espacos_de_recuo() {
        fn arquivos(dir: &std::path::Path, saida: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    arquivos(&p, saida);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    saida.push(p);
                }
            }
        }
        let mut lista = Vec::new();
        arquivos(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut lista,
        );
        assert!(lista.len() > 10);
        for arquivo in lista {
            let texto = std::fs::read_to_string(&arquivo).unwrap();
            for (n, linha) in texto.lines().enumerate() {
                // inside a string literal: an odd number of quotes before the gap
                if let Some(pos) = linha.trim_end().find("      ") {
                    let antes = &linha[..pos];
                    let tem_texto_antes =
                        antes.trim_end().len() == antes.len() && !antes.trim().is_empty();
                    if tem_texto_antes && antes.matches('"').count() % 2 == 1 {
                        panic!(
                            "{}:{}: spaces inside a text: {}",
                            arquivo.display(),
                            n + 1,
                            linha.trim()
                        );
                    }
                }
            }
        }
    }
}
