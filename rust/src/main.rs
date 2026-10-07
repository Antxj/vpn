//! VPN - OpenVPN connections (one or several at the same time) with a TOTP
//! token generated automatically.
#![windows_subsystem = "windows"]

// first: the tr!/trf! macros must be defined before the modules
#[macro_use]
mod i18n;
mod update;
mod accounts;
mod dpapi;
mod elevate;
mod state;
mod installer;
mod engine;
mod qr;
mod routes;
mod service;
mod single;
mod startup;
mod totp;
mod vpn;

use accounts::{Autenticacao, Conta};
use eframe::egui;
use state::{Agregado, Situacao};
use engine::ErroConexao;
use std::cell::RefCell;
use std::ffi::c_void;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconGetRect, Shell_NotifyIconW, NIF_INFO, NIIF_INFO, NIM_MODIFY,
    NOTIFYICONDATAW, NOTIFYICONIDENTIFIER,
};

const APP_TITLE: &str = "VPN";
const WINDOW_TITLE: &str = concat!("VPN v", env!("CARGO_PKG_VERSION"));
const OPENVPN_DOWNLOAD_URL: &str = "https://openvpn.net/community-downloads/";

const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x3b, 0x82, 0xf6);
/// Window size (fixed: the window is not resizable).
const WINDOW_SIZE: [f32; 2] = [480.0, 660.0];
const OPENVPN_CHECK_INTERVAL: Duration = Duration::from_secs(5);

static MAIN_WINDOW_VISIBLE: AtomicBool = AtomicBool::new(true);
static TRAY_HINT_SHOWN: AtomicBool = AtomicBool::new(false);
static MAIN_WNDPROC_INSTALLED: AtomicBool = AtomicBool::new(false);
static ORIGINAL_MAIN_WNDPROC: AtomicIsize = AtomicIsize::new(0);
static ORIGINAL_MAIN_EXSTYLE: AtomicIsize = AtomicIsize::new(0);
/// Request from the tray menu to open the update window.
static ABRIR_ATUALIZACAO: AtomicBool = AtomicBool::new(false);
/// Started by Windows with "start minimized": hide the window on the first frame.
static INICIAR_OCULTO: AtomicBool = AtomicBool::new(false);
/// This start came from an update done by the app itself.
static APOS_ATUALIZAR: AtomicBool = AtomicBool::new(false);
/// Accounts that were on before the update (turned back on at start).
static RECONECTAR: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();

const OPENVPN_CANDIDATES: &[&str] = &[
    r"C:\Program Files\OpenVPN\bin\openvpn.exe",
    r"C:\Program Files (x86)\OpenVPN\bin\openvpn.exe",
];
/// Key written by the OpenVPN installer (finds installations outside the
/// default path, e.g. on another drive).
const OPENVPN_REG_KEY: &str = r"SOFTWARE\OpenVPN";

// -------------------------------------------------------- Win32 (window) ---

const SW_HIDE: i32 = 0;
const SW_SHOW: i32 = 5;
const SW_RESTORE: i32 = 9;
const GWL_EXSTYLE: i32 = -20;
const GWLP_WNDPROC: i32 = -4;
const WM_CLOSE: u32 = 0x0010;
const WM_SYSCOMMAND: u32 = 0x0112;
const WM_TIMER: u32 = 0x0113;
const SC_MINIMIZE: usize = 0xF020;
const DWMWA_CLOAK: u32 = 13;
const WS_EX_TOOLWINDOW: isize = 0x0000_0080;
const WS_EX_APPWINDOW: isize = 0x0004_0000;
const SWP_NOSIZE: u32 = 0x0001;
const SWP_NOMOVE: u32 = 0x0002;
const SWP_NOZORDER: u32 = 0x0004;
const SWP_NOACTIVATE: u32 = 0x0010;
const SWP_FRAMECHANGED: u32 = 0x0020;

#[link(name = "user32")]
extern "system" {
    fn FindWindowExW(
        parent: *mut c_void,
        child_after: *mut c_void,
        class: *const u16,
        title: *const u16,
    ) -> *mut c_void;
    fn ShowWindow(hwnd: *mut c_void, cmd: i32) -> i32;
    fn SetForegroundWindow(hwnd: *mut c_void) -> i32;
    fn GetWindowThreadProcessId(hwnd: *mut c_void, process_id: *mut u32) -> u32;
    fn GetWindowLongPtrW(hwnd: *mut c_void, index: i32) -> isize;
    fn SetWindowLongPtrW(hwnd: *mut c_void, index: i32, new_long: isize) -> isize;
    fn SetWindowPos(
        hwnd: *mut c_void,
        insert_after: *mut c_void,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
        flags: u32,
    ) -> i32;
    fn CallWindowProcW(
        previous: *const c_void,
        hwnd: *mut c_void,
        msg: u32,
        wparam: usize,
        lparam: isize,
    ) -> isize;
    fn RegisterClassW(class: *const WndClassW) -> u16;
    fn CreateWindowExW(
        ex_style: u32,
        class_name: *const u16,
        window_name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: *mut c_void,
        menu: *mut c_void,
        instance: *mut c_void,
        param: *mut c_void,
    ) -> *mut c_void;
    fn DefWindowProcW(hwnd: *mut c_void, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn SetTimer(hwnd: *mut c_void, id: usize, elapse_ms: u32, callback: *const c_void) -> usize;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn GetCurrentProcessId() -> u32;
}

#[link(name = "dwmapi")]
extern "system" {
    fn DwmSetWindowAttribute(
        hwnd: *mut c_void,
        attribute: u32,
        value: *const c_void,
        value_size: u32,
    ) -> i32;
}

#[repr(C)]
struct WndClassW {
    style: u32,
    wndproc: unsafe extern "system" fn(*mut c_void, u32, usize, isize) -> isize,
    cls_extra: i32,
    wnd_extra: i32,
    hinstance: *mut c_void,
    hicon: *mut c_void,
    hcursor: *mut c_void,
    hbrush: *mut c_void,
    menu_name: *const u16,
    class_name: *const u16,
}

/// Hides/shows the window via DWM (cloaking). Returns false if DWM refused -
/// in that case the caller has to hide it the classic way, otherwise the
/// window would stay visible.
unsafe fn set_main_window_cloaked(hwnd: *mut c_void, cloaked: bool) -> bool {
    let value = i32::from(cloaked);
    let hr = DwmSetWindowAttribute(
        hwnd,
        DWMWA_CLOAK,
        &value as *const i32 as *const c_void,
        std::mem::size_of::<i32>() as u32,
    );
    hr == 0
}

unsafe fn set_main_window_taskbar(hwnd: *mut c_void, visible: bool) {
    let original = ORIGINAL_MAIN_EXSTYLE.load(Ordering::SeqCst);
    let style = if visible {
        original
    } else {
        (original & !WS_EX_APPWINDOW) | WS_EX_TOOLWINDOW
    };

    ShowWindow(hwnd, SW_HIDE);
    SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style);
    SetWindowPos(
        hwnd,
        ptr::null_mut(),
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
    );
    ShowWindow(hwnd, SW_SHOW);
}

unsafe fn find_own_tray_window() -> *mut c_void {
    // Internal class of the tray-icon crate (pinned by Cargo.lock). If it ever
    // changes, the balloon notification silently stops showing up - the rest
    // of the tray keeps working.
    let class = single::wide("tray_icon_app");
    let process_id = GetCurrentProcessId();
    let mut hwnd = ptr::null_mut();

    loop {
        hwnd = FindWindowExW(ptr::null_mut(), hwnd, class.as_ptr(), ptr::null());
        if hwnd.is_null() {
            return ptr::null_mut();
        }

        let mut owner_process_id = 0;
        GetWindowThreadProcessId(hwnd, &mut owner_process_id);
        if owner_process_id == process_id {
            return hwnd;
        }
    }
}

fn copy_notification_text<const N: usize>(target: &mut [u16; N], text: &str) {
    for (target, value) in target
        .iter_mut()
        .zip(text.encode_utf16().take(N.saturating_sub(1)))
    {
        *target = value;
    }
}

unsafe fn show_tray_notification() -> bool {
    let hwnd = find_own_tray_window();
    if hwnd.is_null() {
        return false;
    }

    for icon_id in 1..=32 {
        let mut identifier: NOTIFYICONIDENTIFIER = std::mem::zeroed();
        identifier.cbSize = std::mem::size_of::<NOTIFYICONIDENTIFIER>() as u32;
        identifier.hWnd = hwnd;
        identifier.uID = icon_id;

        let mut rect: RECT = std::mem::zeroed();
        if Shell_NotifyIconGetRect(&identifier, &mut rect) != 0 {
            continue;
        }

        let mut notification: NOTIFYICONDATAW = std::mem::zeroed();
        notification.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        notification.hWnd = hwnd;
        notification.uID = icon_id;
        notification.uFlags = NIF_INFO;
        notification.dwInfoFlags = NIIF_INFO;
        notification.Anonymous.uTimeout = 4_000;
        copy_notification_text(&mut notification.szInfoTitle, APP_TITLE);
        copy_notification_text(
            &mut notification.szInfo,
            tr!(
                "O aplicativo VPN continua ativo na bandeja. Clique no ícone para reabrir.",
                "VPN is still running in the system tray. Click the icon to reopen it."
            ),
        );
        return Shell_NotifyIconW(NIM_MODIFY, &notification) != 0;
    }

    false
}

/// Main window of this process. Searches by title AND by process: with a
/// generic name like "VPN", another program could have a window with the
/// same title.
unsafe fn find_main_window() -> *mut c_void {
    let title = single::wide(WINDOW_TITLE);
    let process_id = GetCurrentProcessId();
    let mut hwnd = ptr::null_mut();
    loop {
        hwnd = FindWindowExW(ptr::null_mut(), hwnd, ptr::null(), title.as_ptr());
        if hwnd.is_null() {
            return hwnd;
        }
        let mut owner_process_id = 0;
        GetWindowThreadProcessId(hwnd, &mut owner_process_id);
        if owner_process_id == process_id {
            return hwnd;
        }
    }
}

/// Restores the main window straight through the Windows API.
/// Works even with the egui loop paused (window hidden).
pub fn show_main_window() {
    MAIN_WINDOW_VISIBLE.store(true, Ordering::SeqCst);
    unsafe {
        let hwnd = find_main_window();
        if !hwnd.is_null() {
            // started hidden (tray only): the hook that saves the original
            // style is not installed yet, and there is nothing to restore
            if MAIN_WNDPROC_INSTALLED.load(Ordering::SeqCst) {
                set_main_window_taskbar(hwnd, true);
            }
            let _ = set_main_window_cloaked(hwnd, false);
            ShowWindow(hwnd, SW_RESTORE);
            ShowWindow(hwnd, SW_SHOW);
            SetForegroundWindow(hwnd);
        }
    }
    if let Some(ctx) = EGUI_CTX.get() {
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.request_repaint();
    }
}

/// Hides the main window to the tray (no taskbar button). Also used when
/// Windows starts the app minimized.
unsafe fn esconder_janela(hwnd: *mut c_void) {
    let cloaked = set_main_window_cloaked(hwnd, true);
    set_main_window_taskbar(hwnd, false);
    if !cloaked {
        // DWM unavailable (rare): set_main_window_taskbar ends with
        // SW_SHOW, so without this line the window would reappear
        ShowWindow(hwnd, SW_HIDE);
    }
    MAIN_WINDOW_VISIBLE.store(false, Ordering::SeqCst);
}

unsafe extern "system" fn main_wnd_proc(
    hwnd: *mut c_void,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    if msg == WM_CLOSE || (msg == WM_SYSCOMMAND && wparam & 0xFFF0 == SC_MINIMIZE) {
        esconder_janela(hwnd);
        if !TRAY_HINT_SHOWN.load(Ordering::SeqCst)
            && std::env::var_os("VPN_SKIP_HINT").is_none()
        {
            if show_tray_notification() {
                TRAY_HINT_SHOWN.store(true, Ordering::SeqCst);
            }
        }
        return 0;
    }

    let previous = ORIGINAL_MAIN_WNDPROC.load(Ordering::SeqCst);
    if previous != 0 {
        CallWindowProcW(previous as *const c_void, hwnd, msg, wparam, lparam)
    } else {
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}

fn install_main_window_hook() {
    if MAIN_WNDPROC_INSTALLED.load(Ordering::SeqCst) {
        return;
    }
    unsafe {
        let hwnd = find_main_window();
        if !hwnd.is_null() {
            ORIGINAL_MAIN_EXSTYLE.store(GetWindowLongPtrW(hwnd, GWL_EXSTYLE), Ordering::SeqCst);
            let previous = SetWindowLongPtrW(
                hwnd,
                GWLP_WNDPROC,
                main_wnd_proc as *const () as usize as isize,
            );
            if previous != 0 {
                ORIGINAL_MAIN_WNDPROC.store(previous, Ordering::SeqCst);
                MAIN_WNDPROC_INSTALLED.store(true, Ordering::SeqCst);
            }
        }
    }
}

// ------------------------------------------------------------- utilities ---

/// Reads HKLM\SOFTWARE\OpenVPN\exe_path (written by the official installer).
fn openvpn_from_registry() -> Option<PathBuf> {
    use windows_sys::Win32::System::Registry::{
        RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6432KEY,
        RRF_SUBKEY_WOW6464KEY,
    };

    let key = single::wide(OPENVPN_REG_KEY);
    let value = single::wide("exe_path");
    // tries the 64-bit and the 32-bit registry views
    for view in [RRF_SUBKEY_WOW6464KEY, RRF_SUBKEY_WOW6432KEY] {
        let mut buf = [0u16; 512];
        let mut len = (buf.len() * 2) as u32;
        let ok = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ | view,
                std::ptr::null_mut(),
                buf.as_mut_ptr() as *mut c_void,
                &mut len,
            )
        };
        if ok == 0 {
            let chars = (len as usize / 2).saturating_sub(1).min(buf.len());
            let path = PathBuf::from(String::from_utf16_lossy(&buf[..chars]));
            if path.exists() {
                return Some(path);
            }
        }
    }
    None
}

fn find_openvpn() -> Option<PathBuf> {
    // VPN_OPENVPN points to an openvpn.exe outside the default path (and,
    // with a non-existent path, tests the missing-OpenVPN notice)
    if let Ok(custom) = std::env::var("VPN_OPENVPN") {
        let p = PathBuf::from(custom);
        return if p.exists() { Some(p) } else { None };
    }
    OPENVPN_CANDIDATES
        .iter()
        .map(PathBuf::from)
        .find(|p| p.exists())
        .or_else(openvpn_from_registry)
}

fn config_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(profile) = std::env::var("USERPROFILE") {
        dirs.push(PathBuf::from(profile).join("OpenVPN").join("config"));
    }
    dirs.push(PathBuf::from(r"C:\Program Files\OpenVPN\config"));
    dirs
}

fn find_default_config() -> Option<PathBuf> {
    for dir in config_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut ovpns: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("ovpn")))
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

/// Color of the field labels (more readable than egui's default "weak").
fn label_color(dark: bool) -> egui::Color32 {
    if dark {
        egui::Color32::from_gray(185)
    } else {
        egui::Color32::from_gray(95)
    }
}

fn error_box(msg: &str) {
    rfd::MessageDialog::new()
        .set_title(APP_TITLE)
        .set_description(msg)
        .set_level(rfd::MessageLevel::Error)
        .show();
}

fn report_startup_error(error: &eframe::Error) {
    let app_dir = dpapi::app_dir();
    let log_path = app_dir.join("startup-error.log");
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let entry = format!(
        "[unix {timestamp}] {WINDOW_TITLE}\nErro: {error}\nDetalhes: {error:#?}\n\n"
    );

    let log_result = std::fs::create_dir_all(&app_dir).and_then(|_| {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?
            .write_all(entry.as_bytes())
    });

    let log_info = match log_result {
        Ok(()) => trf!("Detalhes registrados em:\n{}", "Details logged in:\n{}", log_path.display()),
        Err(log_error) => trf!(
            "Não foi possível gravar o log: {log_error}",
            "Could not write the log: {log_error}"
        ),
    };
    error_box(&trf!(
        "Não foi possível iniciar a interface gráfica.\n\nErro: {error}\n\n{log_info}",
        "Could not start the user interface.\n\nError: {error}\n\n{log_info}"
    ));
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

fn apply_style(ctx: &egui::Context, dark: bool) {
    let mut visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    let rounding = egui::Rounding::same(6.0);
    visuals.widgets.noninteractive.rounding = rounding;
    visuals.widgets.inactive.rounding = rounding;
    visuals.widgets.hovered.rounding = rounding;
    visuals.widgets.active.rounding = rounding;
    visuals.widgets.open.rounding = rounding;
    visuals.selection.bg_fill = ACCENT;
    visuals.hyperlink_color = ACCENT;

    let mut style = (*ctx.style()).clone();
    style.visuals = visuals;
    use egui::{FontFamily, FontId, TextStyle};
    style.text_styles = [
        (TextStyle::Heading, FontId::new(24.0, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(15.0, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(15.0, FontFamily::Proportional)),
        (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(12.5, FontFamily::Monospace)),
    ]
    .into();
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(14.0, 8.0);
    // egui's default (500 px) is wider than the 480 px window: long tooltips
    // were cut at the window edge
    style.spacing.tooltip_width = 320.0;
    ctx.set_style(style);
}

// --------------------------------- tray (main/Win32 thread only) ---

#[derive(Clone)]
enum AcaoMenu {
    Abrir,
    Atualizar,
    Alternar(String),
    DesconectarTodas,
    Sair,
}

/// Action of each tray menu item. The menu is rebuilt when the accounts
/// change; the handler (which runs outside the egui loop) looks up this table.
static MENU_ACOES: Mutex<Vec<(MenuId, AcaoMenu)>> = Mutex::new(Vec::new());

struct TrayUi {
    tray: TrayIcon,
    icons: [tray_icon::Icon; 3], // gray, amber, green
    /// Checkable item of each account, to reflect connected/disconnected.
    itens: Vec<(String, CheckMenuItem)>,
    /// (id, name) of the accounts, new version offered and language of the
    /// current menu; None = menu not built yet.
    assinatura: Option<(Vec<(String, String)>, Option<String>, bool)>,
    last_icon: usize,
    last_tip: String,
}

thread_local! {
    static TRAY_UI: RefCell<Option<TrayUi>> = const { RefCell::new(None) };
}

fn montar_menu(
    contas: &[(String, String)],
    versao_nova: Option<&str>,
) -> (Menu, Vec<(String, CheckMenuItem)>, Vec<(MenuId, AcaoMenu)>) {
    let menu = Menu::new();
    let mut acoes = Vec::new();
    let mut itens = Vec::new();

    let abrir = MenuItem::new(tr!("Abrir", "Open"), true, None);
    acoes.push((abrir.id().clone(), AcaoMenu::Abrir));
    let _ = menu.append(&abrir);
    if let Some(v) = versao_nova {
        let texto = trf!("Atualizar para a versão {v}...", "Update to version {v}...");
        let item = MenuItem::new(texto, true, None);
        acoes.push((item.id().clone(), AcaoMenu::Atualizar));
        let _ = menu.append(&item);
    }
    let _ = menu.append(&PredefinedMenuItem::separator());

    if contas.is_empty() {
        let vazio = tr!("Nenhuma conta cadastrada", "No accounts yet");
        let _ = menu.append(&MenuItem::new(vazio, false, None));
    }
    for (id, nome) in contas {
        let item = CheckMenuItem::new(nome, true, false, None);
        acoes.push((item.id().clone(), AcaoMenu::Alternar(id.clone())));
        let _ = menu.append(&item);
        itens.push((id.clone(), item));
    }
    if contas.len() > 1 {
        let todas = MenuItem::new(tr!("Desconectar todas", "Disconnect all"), true, None);
        acoes.push((todas.id().clone(), AcaoMenu::DesconectarTodas));
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&todas);
    }

    let _ = menu.append(&PredefinedMenuItem::separator());
    let sair = MenuItem::new(tr!("Sair", "Exit"), true, None);
    acoes.push((sair.id().clone(), AcaoMenu::Sair));
    let _ = menu.append(&sair);
    (menu, itens, acoes)
}

/// Account "on" for toggle/check purposes: connection in progress that is
/// not being shut down.
fn conta_ligada(id: &str) -> bool {
    engine::get().ativa(id) && state::obter(id).situacao != Situacao::Desconectando
}

/// Updates the tray icon, tooltip and menu from the shared state.
/// Runs on the main thread: called by the App and by the Win32 timer (which
/// works with the window hidden).
fn apply_tray_state() {
    let nomes = engine::get().nomes();
    let versao_nova = update::disponivel();
    TRAY_UI.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let Some(ui) = borrow.as_mut() else { return };

        let assinatura = (nomes.clone(), versao_nova.clone(), i18n::pt());
        if ui.assinatura.as_ref() != Some(&assinatura) {
            let (menu, itens, acoes) = montar_menu(&nomes, versao_nova.as_deref());
            ui.tray.set_menu(Some(Box::new(menu)));
            ui.itens = itens;
            *MENU_ACOES.lock().unwrap() = acoes;
            ui.assinatura = Some(assinatura);
        }
        for (id, item) in &ui.itens {
            let ligada = conta_ligada(id);
            if item.is_checked() != ligada {
                item.set_checked(ligada);
            }
        }

        let (agregado, linhas) = state::resumo(&nomes);
        let icon_idx = match agregado {
            Agregado::Conectado => 2,
            Agregado::Transicao => 1,
            Agregado::Nenhuma => 0,
        };
        // Windows limit for the tray tooltip: 127 characters
        let tip: String = format!("{APP_TITLE} - {}", linhas.join("\n"))
            .chars()
            .take(127)
            .collect();

        if ui.last_icon != icon_idx {
            let _ = ui.tray.set_icon(Some(ui.icons[icon_idx].clone()));
            ui.last_icon = icon_idx;
        }
        if ui.last_tip != tip {
            let _ = ui.tray.set_tooltip(Some(&tip));
            ui.last_tip = tip;
        }
    });
}

unsafe extern "system" fn tick_wnd_proc(
    hwnd: *mut c_void,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    if msg == WM_TIMER {
        apply_tray_state();
        return 0;
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// Invisible window with a 1 s timer: keeps the tray (icon/tooltip/menu)
/// up to date even when the egui loop is paused (window hidden).
fn create_tick_window() {
    let class_name: &'static [u16] = Box::leak(single::wide("VpnAppTick").into_boxed_slice());
    unsafe {
        let hinstance = GetModuleHandleW(ptr::null());
        let wc = WndClassW {
            style: 0,
            wndproc: tick_wnd_proc,
            cls_extra: 0,
            wnd_extra: 0,
            hinstance,
            hicon: ptr::null_mut(),
            hcursor: ptr::null_mut(),
            hbrush: ptr::null_mut(),
            menu_name: ptr::null(),
            class_name: class_name.as_ptr(),
        };
        RegisterClassW(&wc);
        let hwnd_message = -3isize as *mut c_void;
        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            ptr::null(),
            0,
            0,
            0,
            0,
            0,
            hwnd_message,
            ptr::null_mut(),
            hinstance,
            ptr::null_mut(),
        );
        if !hwnd.is_null() {
            SetTimer(hwnd, 1, 1000, ptr::null());
        }
    }
}

/// Turns an account on or off. Used by the UI and by the tray menu.
/// Asks for confirmation when two connections would fight over the default route.
fn alternar_conta(id: &str) -> Result<(), ErroConexao> {
    let m = engine::get();
    if m.ativa(id) {
        m.desconectar(id);
        return Ok(());
    }
    let conflitos = m.conflitos_de_rota(id);
    if !conflitos.is_empty() {
        let nome = m.conta(id).map(|c| c.nome_exibicao().to_string()).unwrap_or_default();
        let resposta = rfd::MessageDialog::new()
            .set_title(APP_TITLE)
            .set_description(trf!(
                "\"{nome}\" e \"{}\" mandam todo o tráfego da internet pela VPN.\n\n\
                 Com as duas conectadas, só a última funciona como rota padrão \
                 (a outra continua acessando apenas a própria rede). Conectar mesmo assim?",
                "\"{nome}\" and \"{}\" both send all internet traffic through the VPN.\n\n\
                 With both connected, only the last one works as the default route \
                 (the other one keeps reaching only its own network). Connect anyway?",
                conflitos.join("\", \"")
            ))
            .set_level(rfd::MessageLevel::Warning)
            .set_buttons(rfd::MessageButtons::YesNo)
            .show();
        if resposta != rfd::MessageDialogResult::Yes {
            return Ok(());
        }
    }
    m.conectar(id, find_openvpn())
}

fn install_tray_handlers() {
    MenuEvent::set_event_handler(Some(|ev: MenuEvent| {
        let acao = MENU_ACOES
            .lock()
            .unwrap()
            .iter()
            .find(|(id, _)| *id == ev.id)
            .map(|(_, a)| a.clone());
        match acao {
            Some(AcaoMenu::Abrir) => show_main_window(),
            Some(AcaoMenu::Atualizar) => {
                ABRIR_ATUALIZACAO.store(true, Ordering::SeqCst);
                show_main_window();
            }
            Some(AcaoMenu::Alternar(id)) => {
                if let Err(e) = alternar_conta(&id) {
                    // without the window the installation cannot be offered: open it,
                    // where the yellow notice has the "Install now" button
                    show_main_window();
                    error_box(&e.mensagem());
                }
            }
            Some(AcaoMenu::DesconectarTodas) => engine::get().desconectar_todas(),
            Some(AcaoMenu::Sair) => {
                // Exit for sure: disconnects (up to 15 s) and ends the process,
                // without depending on the egui loop being awake.
                std::thread::spawn(|| engine::get().encerrar());
            }
            None => {}
        }
    }));
    TrayIconEvent::set_event_handler(Some(|ev: TrayIconEvent| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = ev
        {
            show_main_window();
        }
    }));
}

/// Shows an error without blocking the caller (connection threads).
pub fn error_box_async(msg: String) {
    if cfg!(test) {
        state::log("", msg); // no modal windows during tests
        return;
    }
    std::thread::spawn(move || error_box(&msg));
}

/// Drawn toggle switch: on = blue.
fn toggle(ui: &mut egui::Ui, ligado: bool, habilitado: bool) -> egui::Response {
    let tamanho = egui::vec2(44.0, 24.0);
    let sentido = if habilitado { egui::Sense::click() } else { egui::Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(tamanho, sentido);
    if ui.is_rect_visible(rect) {
        let anim = ui.ctx().animate_bool(response.id, ligado);
        let fundo = if ligado {
            ACCENT
        } else if ui.visuals().dark_mode {
            egui::Color32::from_gray(70)
        } else {
            egui::Color32::from_gray(190)
        };
        let fundo = if habilitado { fundo } else { fundo.gamma_multiply(0.35) };
        let raio = rect.height() / 2.0;
        ui.painter().rect_filled(rect, raio, fundo);
        let x = egui::lerp((rect.left() + raio)..=(rect.right() - raio), anim);
        ui.painter().circle_filled(
            egui::pos2(x, rect.center().y),
            raio - 3.0,
            egui::Color32::WHITE,
        );
    }
    if habilitado {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    }
}

/// Account tunnel type, for the card: (short text, explanation).
fn tipo_de_tunel(conta: &Conta) -> Option<(&'static str, &'static str)> {
    conta.tunel_completo().map(|completo| {
        if completo {
            (
                tr!("toda a internet", "all traffic"),
                tr!(
                    "Toda a internet passa por esta VPN (túnel completo).",
                    "All internet traffic goes through this VPN (full tunnel)."
                ),
            )
        } else {
            (
                tr!("só a rede da VPN", "VPN network only"),
                tr!(
                    "Só a rede da VPN passa por ela; o resto usa a sua internet \
                     normal (túnel dividido).",
                    "Only the VPN's network goes through it; everything else uses \
                     your regular internet (split tunnel)."
                ),
            )
        }
    })
}

/// The .ovpn decides whether there is a username/password at all: without
/// `auth-user-pass` the account becomes "certificate only".
fn ajustar_autenticacao(conta: &mut Conta) {
    if conta.config.is_empty() {
        return;
    }
    let pede = accounts::pede_usuario_e_senha(std::path::Path::new(&conta.config));
    if !pede {
        conta.autenticacao = Autenticacao::SoCertificado;
    } else if conta.autenticacao == Autenticacao::SoCertificado {
        conta.autenticacao = Autenticacao::Token;
    }
}

fn cor_situacao(s: Situacao, padrao: egui::Color32) -> egui::Color32 {
    match s {
        Situacao::Conectado => egui::Color32::from_rgb(0x2a, 0xa0, 0x2a),
        Situacao::Conectando | Situacao::Reconectando | Situacao::Desconectando => {
            egui::Color32::from_rgb(0xb5, 0x89, 0x00)
        }
        Situacao::Desconectado => padrao,
    }
}

/// Accounts that appear in the log, in the order of the account list.
fn abas_do_log(contas: &[Conta]) -> Vec<String> {
    let entradas = state::log_entradas();
    contas
        .iter()
        .map(|c| c.nome_exibicao().to_string())
        .filter(|n| entradas.iter().any(|e| &e.origem == n))
        .collect()
}

/// Soft color of each account in the log (by position in the list).
fn cor_da_conta(i: usize, dark: bool) -> egui::Color32 {
    const ESCURO: [(u8, u8, u8); 5] =
        [(0x60, 0xa5, 0xfa), (0x34, 0xd3, 0x99), (0xfb, 0xbf, 0x24), (0xf4, 0x72, 0xb6), (0xa7, 0x8b, 0xfa)];
    const CLARO: [(u8, u8, u8); 5] =
        [(0x1d, 0x4e, 0xd8), (0x04, 0x78, 0x57), (0xb4, 0x53, 0x09), (0xbe, 0x18, 0x5d), (0x6d, 0x28, 0xd9)];
    let (r, g, b) = if dark { ESCURO[i % 5] } else { CLARO[i % 5] };
    egui::Color32::from_rgb(r, g, b)
}

/// Discreet log tab: small text, underlined when selected. Returns true when clicked.
fn aba(ui: &mut egui::Ui, texto: &str, ativa: bool, cor: Option<egui::Color32>, dark: bool) -> bool {
    let cor_texto = if ativa {
        cor.unwrap_or_else(|| ui.visuals().strong_text_color())
    } else {
        label_color(dark)
    };
    let mut rich = egui::RichText::new(texto).small().color(cor_texto);
    if ativa {
        rich = rich.strong();
    }
    let r = ui
        .add(egui::Label::new(rich).sense(egui::Sense::click()))
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if ativa {
        let y = r.rect.bottom() + 2.0;
        ui.painter().line_segment(
            [egui::pos2(r.rect.left(), y), egui::pos2(r.rect.right(), y)],
            egui::Stroke::new(2.0_f32, cor.unwrap_or(ACCENT)),
        );
    }
    r.clicked()
}

/// Background of the cards (accounts, settings sections).
fn fundo_cartao(dark: bool) -> egui::Color32 {
    if dark {
        egui::Color32::from_rgb(0x26, 0x29, 0x31)
    } else {
        egui::Color32::from_rgb(0xff, 0xff, 0xff)
    }
}

/// Settings section: small title above a card.
fn secao(ui: &mut egui::Ui, titulo: &str, dark: bool, conteudo: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(8.0);
    rotulo(ui, titulo, dark);
    egui::Frame::none()
        .fill(fundo_cartao(dark))
        .rounding(8.0)
        .inner_margin(egui::Margin::symmetric(12.0, 10.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            conteudo(ui);
        });
}

/// Settings row: title and description on the left, switch on the right.
/// Returns true when the user flipped the switch.
fn linha_toggle(
    ui: &mut egui::Ui,
    dark: bool,
    titulo: &str,
    descricao: &str,
    valor: &mut bool,
    habilitado: bool,
) -> bool {
    let mut mudou = false;
    ui.horizontal(|ui| {
        let largura = (ui.available_width() - 64.0).max(80.0);
        ui.vertical(|ui| {
            ui.set_max_width(largura);
            let cor = if habilitado {
                ui.visuals().strong_text_color()
            } else if dark {
                egui::Color32::from_gray(110)
            } else {
                egui::Color32::from_gray(165)
            };
            ui.add(egui::Label::new(egui::RichText::new(titulo).color(cor)).truncate());
            if !descricao.is_empty() {
                ui.label(egui::RichText::new(descricao).small().color(label_color(dark)));
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if toggle(ui, *valor, habilitado).clicked() {
                *valor = !*valor;
                mudou = true;
            }
        });
    });
    mudou
}

fn rotulo(ui: &mut egui::Ui, texto: &str, dark: bool) {
    ui.label(egui::RichText::new(texto).small().color(label_color(dark)));
}

// --------------------------------------------------------------------- app ---

#[derive(PartialEq)]
enum Tela {
    Inicio,
    Contas,
    Configuracoes,
}

/// Account being created or edited on the accounts screen.
struct Editor {
    conta: Conta,
    nova: bool,
    mostrar_seed: bool,
    mostrar_senha: bool,
    erro: Option<String>,
}

struct App {
    dark: bool,
    tela: Tela,
    editor: Option<Editor>,
    qr_open: bool,
    /// Log tab: None = all accounts, Some(name) = only that account.
    aba_log: Option<String>,
    janela_atualizacao: bool,
    openvpn_missing: bool,
    last_ovpn_check: Instant,
    installing: bool,
    install_rx: Receiver<installer::Event>,
    install_tx: Sender<installer::Event>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let m = engine::get();
        let dark = m.tema_escuro();
        apply_style(&cc.egui_ctx, dark);
        let _ = EGUI_CTX.set(cc.egui_ctx.clone());
        let ctx = cc.egui_ctx.clone();
        state::ao_mudar(move || ctx.request_repaint());

        single::spawn_show_listener();

        // ---- tray ----
        let (g, gw, gh) = load_icon_rgba(include_bytes!("../assets/gray_32.png"));
        let (w_, ww, wh) = load_icon_rgba(include_bytes!("../assets/warn_32.png"));
        let (o, ow, oh) = load_icon_rgba(include_bytes!("../assets/ok_32.png"));
        let icon_gray = tray_icon::Icon::from_rgba(g, gw, gh).unwrap();
        let icon_warn = tray_icon::Icon::from_rgba(w_, ww, wh).unwrap();
        let icon_ok = tray_icon::Icon::from_rgba(o, ow, oh).unwrap();

        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(Menu::new()))
            .with_menu_on_left_click(false)
            .with_tooltip(format!("{APP_TITLE} - {}", tr!("Desconectado", "Disconnected")))
            .with_icon(icon_gray.clone())
            .build()
            .expect("failed to create the tray icon");
        install_tray_handlers();

        TRAY_UI.with(|cell| {
            *cell.borrow_mut() = Some(TrayUi {
                tray,
                icons: [icon_gray, icon_warn, icon_ok],
                itens: Vec::new(),
                assinatura: None,
                last_icon: 0,
                last_tip: String::new(),
            });
        });
        apply_tray_state();
        create_tick_window();

        if INICIAR_OCULTO.load(Ordering::SeqCst) {
            // cloaked before the first paint, so the window never flashes;
            // the rest of the hiding happens on the first frame
            unsafe {
                let hwnd = find_main_window();
                if !hwnd.is_null() {
                    set_main_window_cloaked(hwnd, true);
                }
            }
        }

        let (install_tx, install_rx) = std::sync::mpsc::channel();
        if APOS_ATUALIZAR.load(Ordering::SeqCst) {
            state::log(
                "",
                trf!(
                    "Aplicativo atualizado para a versão {}.",
                    "App updated to version {}.",
                    update::VERSAO_ATUAL
                ),
            );
        }
        // turn back on the accounts that were connected before the update
        for id in RECONECTAR.get().into_iter().flatten() {
            if let Err(e) = m.conectar(id, find_openvpn()) {
                let nome = m.conta(id).map(|c| c.nome_exibicao().to_string()).unwrap_or_default();
                state::log(
                    &nome,
                    trf!("Não reconectou: {}", "Did not reconnect: {}", e.mensagem()),
                );
            }
        }
        // accounts marked "connect on open", in list order (no dialogs at
        // startup: whatever cannot connect only goes to the log)
        for conta in m.contas().into_iter().filter(|c| c.conectar_ao_abrir) {
            if m.ativa(&conta.id) {
                continue;
            }
            let nome = conta.nome_exibicao();
            if !m.conflitos_de_rota(&conta.id).is_empty() {
                state::log(
                    nome,
                    tr!(
                        "Não conectou ao abrir: outra VPN já leva toda a internet.",
                        "Not connected on open: another VPN already carries all traffic."
                    ),
                );
                continue;
            }
            if let Err(e) = m.conectar(&conta.id, find_openvpn()) {
                state::log(nome, trf!("Não conectou ao abrir: {}", "Not connected on open: {}", e.mensagem()));
            }
        }
        let ctx = cc.egui_ctx.clone();
        update::ao_mudar(move || ctx.request_repaint());
        update::iniciar_verificacao_periodica(|| engine::get().verifica_atualizacoes());

        let mut app = Self {
            dark,
            // no accounts: open straight on the sign-up screen
            tela: if m.contas().is_empty() { Tela::Contas } else { Tela::Inicio },
            editor: None,
            qr_open: false,
            aba_log: None,
            janela_atualizacao: false,
            openvpn_missing: find_openvpn().is_none(),
            last_ovpn_check: Instant::now(),
            installing: false,
            install_rx,
            install_tx,
        };
        // VPN_SCREENSHOT=accounts|edit|new opens straight on that screen (documentation
        // screenshots and visual check of each screen)
        match std::env::var("VPN_SCREENSHOT").as_deref() {
            Ok("accounts") => app.tela = Tela::Contas,
            Ok("edit") => {
                if let Some(c) = m.contas().into_iter().next() {
                    app.abrir_editor(c, false);
                }
            }
            Ok("new") => app.abrir_editor(Conta::nova(), true),
            Ok("update") => app.janela_atualizacao = true,
            Ok("settings") => app.tela = Tela::Configuracoes,
            // "log:N": log tab of the N-th account (screenshots)
            Ok(v) if v.starts_with("log:") => {
                let i: usize = v[4..].parse().unwrap_or(0);
                app.aba_log = m.contas().get(i).map(|c| c.nome_exibicao().to_string());
            }
            _ => {}
        }
        app
    }

    /// Installs the embedded OpenVPN (silently, in the background).
    fn start_openvpn_install(&mut self) {
        if self.installing || !installer::is_available() {
            return;
        }
        self.installing = true;
        state::log(
            "",
            trf!(
                "Instalando o OpenVPN Community {}... (pode levar cerca de um minuto)",
                "Installing OpenVPN Community {}... (this may take about a minute)",
                installer::MSI_VERSION
            ),
        );
        installer::install_in_background(self.install_tx.clone(), eframe_ctx());
    }

    /// Shows the connection error; when OpenVPN is missing, offers to install it.
    fn tratar_erro(&mut self, erro: ErroConexao) {
        if erro != ErroConexao::OpenVpnAusente {
            error_box(&erro.mensagem());
            return;
        }
        if self.installing {
            return;
        }
        let (pergunta, instala) = if installer::is_available() {
            (
                trf!(
                    "O OpenVPN Community não está instalado — ele é necessário \
                     para conectar à VPN.\n\nInstalar agora? O aplicativo já traz \
                     o instalador oficial (versão {}) e faz tudo sozinho, sem \
                     precisar baixar nada.",
                    "OpenVPN Community is not installed — it is required to \
                     connect to the VPN.\n\nInstall it now? The app already ships \
                     the official installer (version {}) and does everything by \
                     itself, with nothing to download.",
                    installer::MSI_VERSION
                ),
                true,
            )
        } else {
            (
                tr!(
                    "O OpenVPN Community não está instalado — ele é necessário \
                     para conectar à VPN.\n\nAbrir a página de download agora?",
                    "OpenVPN Community is not installed — it is required to \
                     connect to the VPN.\n\nOpen the download page now?"
                )
                .to_string(),
                false,
            )
        };
        let sim = rfd::MessageDialog::new()
            .set_title(APP_TITLE)
            .set_description(&pergunta)
            .set_level(rfd::MessageLevel::Warning)
            .set_buttons(rfd::MessageButtons::YesNo)
            .show();
        if sim == rfd::MessageDialogResult::Yes {
            if instala {
                self.start_openvpn_install();
            } else {
                let _ = open::that(OPENVPN_DOWNLOAD_URL);
            }
        }
    }

    fn poll_events(&mut self) {
        while let Ok(ev) = self.install_rx.try_recv() {
            match ev {
                installer::Event::Done(result) => {
                    self.installing = false;
                    self.openvpn_missing = find_openvpn().is_none();
                    self.last_ovpn_check = Instant::now();
                    match result {
                        Ok(_) if self.openvpn_missing => {
                            state::log(
                                "",
                                tr!(
                                    "Instalação concluída, mas o OpenVPN não foi encontrado.",
                                    "Installation finished, but OpenVPN was not found."
                                ),
                            );
                            show_main_window();
                            error_box(tr!(
                                "A instalação terminou, mas o OpenVPN não foi encontrado.\n\
                                 Reinicie o computador e abra o aplicativo de novo.",
                                "The installation finished, but OpenVPN was not found.\n\
                                 Restart the computer and open the app again."
                            ));
                        }
                        Ok(reiniciar) => {
                            state::log(
                                "",
                                tr!(
                                    "OpenVPN Community instalado com sucesso.",
                                    "OpenVPN Community installed successfully."
                                ),
                            );
                            if reiniciar {
                                state::log(
                                    "",
                                    tr!(
                                        "O Windows pediu reinicialização; se a conexão falhar, reinicie.",
                                        "Windows requested a restart; if the connection fails, restart."
                                    ),
                                );
                            }
                        }
                        Err(msg) => {
                            state::log("", trf!("Falha na instalação: {msg}", "Installation failed: {msg}"));
                            show_main_window();
                            error_box(&trf!(
                                "Não foi possível instalar o OpenVPN.\n\n{msg}",
                                "Could not install OpenVPN.\n\n{msg}"
                            ));
                        }
                    }
                }
            }
        }

        if self.last_ovpn_check.elapsed() >= OPENVPN_CHECK_INTERVAL {
            self.last_ovpn_check = Instant::now();
            self.openvpn_missing = find_openvpn().is_none();
        }
    }

    // -------------------------------------------------------------- QR code --

    fn import_qr_image(&mut self, img: image::DynamicImage) {
        let Some(text) = qr::decode_qr(&img) else {
            error_box(tr!(
                "Não encontrei um QR Code nessa imagem.\n\
                 Confira se ele aparece inteiro e nítido.",
                "No QR code was found in this image.\n\
                 Make sure it is complete and sharp."
            ));
            return;
        };
        let data = qr::parse_payload(&text);
        let Some(seed) = data.seed else {
            let preview: String = text.chars().take(200).collect();
            error_box(&trf!(
                "Li o QR Code, mas não identifiquei uma seed nele.\n\nConteúdo:\n{preview}",
                "The QR code was read, but no seed was found in it.\n\nContent:\n{preview}"
            ));
            return;
        };
        if let Some(ed) = self.editor.as_mut() {
            ed.conta.seed = seed;
            if let Some(user) = data.user {
                ed.conta.usuario = user;
            }
            ed.erro = None;
        }
        self.qr_open = false;
    }

    fn qr_from_file(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter(tr!("Imagens", "Images"), &["png", "jpg", "jpeg", "bmp", "gif", "webp"])
            .set_title(tr!("Escolha a imagem do QR Code", "Choose the QR code image"))
            .pick_file()
        else {
            return;
        };
        match image::open(&path) {
            Ok(img) => self.import_qr_image(img),
            Err(_) => error_box(tr!("Não consegui abrir essa imagem.", "Could not open this image.")),
        }
    }

    fn qr_from_clipboard(&mut self) {
        let img = arboard::Clipboard::new()
            .ok()
            .and_then(|mut c| c.get_image().ok())
            .and_then(|data| {
                image::RgbaImage::from_raw(
                    data.width as u32,
                    data.height as u32,
                    data.bytes.into_owned(),
                )
                .map(image::DynamicImage::ImageRgba8)
            });
        match img {
            Some(img) => self.import_qr_image(img),
            None => {
                rfd::MessageDialog::new()
                    .set_title(APP_TITLE)
                    .set_description(tr!(
                        "Não há imagem na área de transferência.\n\
                         Copie a imagem do QR Code e tente de novo.",
                        "There is no image on the clipboard.\n\
                         Copy the QR code image and try again."
                    ))
                    .set_level(rfd::MessageLevel::Info)
                    .show();
            }
        }
    }

    // --------------------------------------------------------------- screens --

    fn banners(&mut self, ui: &mut egui::Ui) {
        let (bg, fg) = if self.dark {
            (
                egui::Color32::from_rgb(0x3a, 0x2f, 0x18),
                egui::Color32::from_rgb(0xfb, 0xbf, 0x24),
            )
        } else {
            (
                egui::Color32::from_rgb(0xfe, 0xf3, 0xc7),
                egui::Color32::from_rgb(0x92, 0x40, 0x0e),
            )
        };
        let faixa = |ui: &mut egui::Ui, texto: &str, add: &mut dyn FnMut(&mut egui::Ui)| {
            egui::Frame::none()
                .fill(bg)
                .rounding(6.0)
                .inner_margin(egui::Margin::same(10.0))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.colored_label(fg, texto);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add);
                    });
                });
            ui.add_space(8.0);
        };

        if self.openvpn_missing {
            let embutido = installer::is_available();
            let texto = if self.installing {
                tr!(
                    "Instalando o OpenVPN Community...\nIsso leva cerca de um minuto.",
                    "Installing OpenVPN Community...\nThis takes about a minute."
                )
            } else {
                tr!(
                    "OpenVPN Community não está instalado.\nEle é necessário para conectar à VPN.",
                    "OpenVPN Community is not installed.\nIt is required to connect to the VPN."
                )
            };
            let mut instalar = false;
            let instalando = self.installing;
            faixa(ui, texto, &mut |ui| {
                if instalando {
                    ui.spinner();
                } else {
                    let rotulo = if embutido {
                        tr!("Instalar agora", "Install now")
                    } else {
                        tr!("Baixar", "Download")
                    };
                    let btn = egui::Button::new(
                        egui::RichText::new(rotulo).color(egui::Color32::WHITE),
                    )
                    .fill(ACCENT);
                    if ui.add(btn).clicked() {
                        if embutido {
                            instalar = true;
                        } else {
                            let _ = open::that(OPENVPN_DOWNLOAD_URL);
                        }
                    }
                }
            });
            if instalar {
                self.start_openvpn_install();
            }
        }
    }

    fn tela_inicio(&mut self, ui: &mut egui::Ui) {
        let m = engine::get();
        let contas = m.contas();

        if contas.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new(tr!("Nenhuma conta cadastrada.", "No accounts yet."))
                        .size(16.0),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(tr!(
                        "Cadastre a primeira VPN para conectar.",
                        "Add your first VPN to connect."
                    ))
                        .color(label_color(self.dark)),
                );
                ui.add_space(12.0);
                let btn = egui::Button::new(
                    egui::RichText::new(tr!("Adicionar conta", "Add account"))
                        .color(egui::Color32::WHITE),
                )
                .fill(ACCENT);
                if ui.add(btn).clicked() {
                    self.abrir_editor(Conta::nova(), true);
                }
            });
            return;
        }

        let fundo_cartao = fundo_cartao(self.dark);
        // the list takes the top space and the log is anchored at the bottom
        let varias_ativas = contas.iter().filter(|c| m.ativa(&c.id)).count() > 1;
        let altura_log = 150.0
            + if varias_ativas { 44.0 } else { 0.0 }
            + if abas_do_log(&contas).len() >= 2 { 26.0 } else { 0.0 };
        let altura_lista = (ui.available_height() - altura_log).max(120.0);
        let mut erro: Option<ErroConexao> = None;

        egui::ScrollArea::vertical()
            .max_height(altura_lista)
            .min_scrolled_height(altura_lista)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for conta in &contas {
                    let e = state::obter(&conta.id);
                    let ligada = conta_ligada(&conta.id);
                    egui::Frame::none()
                        .fill(fundo_cartao)
                        .rounding(8.0)
                        .inner_margin(egui::Margin::symmetric(12.0, 10.0))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                // the text never invades the toggle's space:
                                // whatever does not fit ends with an ellipsis
                                let largura_texto = (ui.available_width() - 64.0).max(80.0);
                                let fraco = label_color(self.dark);
                                ui.vertical(|ui| {
                                    ui.set_max_width(largura_texto);
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(conta.nome_exibicao())
                                                .strong()
                                                .size(16.0),
                                        )
                                        .truncate(),
                                    );
                                    let cor = cor_situacao(e.situacao, fraco);
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 6.0;
                                        ui.label(
                                            egui::RichText::new(format!("\u{2022}  {}", e.texto()))
                                                .color(cor),
                                        );
                                        if let Some((tipo, dica)) = tipo_de_tunel(conta) {
                                            ui.label(
                                                egui::RichText::new(format!("·  {tipo}"))
                                                    .small()
                                                    .color(fraco),
                                            )
                                            .on_hover_text(dica);
                                        }
                                    });
                                    if let Some(t) = e.trafego {
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(format!(
                                                    "\u{2B07} {}    \u{2B06} {}",
                                                    fmt_rate(t.down_rate),
                                                    fmt_rate(t.up_rate)
                                                ))
                                                .small()
                                                .color(fraco),
                                            )
                                            .truncate(),
                                        );
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(trf!(
                                                    "recebido {}  ·  enviado {}",
                                                    "received {}  ·  sent {}",
                                                    fmt_bytes(t.down_total as f64),
                                                    fmt_bytes(t.up_total as f64)
                                                ))
                                                .small()
                                                .color(fraco),
                                            )
                                            .truncate(),
                                        );
                                    }
                                });
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let ocupada = e.situacao == Situacao::Desconectando;
                                        let r = toggle(ui, ligada, !ocupada).on_hover_text(
                                            if ligada {
                                                tr!("Desconectar", "Disconnect")
                                            } else {
                                                tr!("Conectar", "Connect")
                                            },
                                        );
                                        if r.clicked() {
                                            if let Err(err) = alternar_conta(&conta.id) {
                                                erro = Some(err);
                                            }
                                        }
                                    },
                                );
                            });
                        });
                    ui.add_space(6.0);
                }
            });

        if let Some(e) = erro {
            self.tratar_erro(e);
        }

        if varias_ativas {
            ui.vertical_centered(|ui| {
                if ui.button(tr!("Desconectar todas", "Disconnect all")).clicked() {
                    m.desconectar_todas();
                }
            });
        }
        ui.add_space(4.0);
        self.painel_log(ui);
    }

    /// Connection log. With two or more accounts in it, discreet tabs on top:
    /// "All" (every account, each name in its own color) or one account only.
    fn painel_log(&mut self, ui: &mut egui::Ui) {
        let dark = self.dark;
        let contas = engine::get().contas();
        let abas = abas_do_log(&contas);
        if self.aba_log.as_ref().is_some_and(|a| !abas.contains(a)) {
            self.aba_log = None;
        }
        let entradas: Vec<state::Entrada> = state::log_entradas()
            .into_iter()
            .filter(|e| self.aba_log.as_ref().is_none_or(|a| &e.origem == a))
            .collect();

        egui::Frame::none()
            .fill(if dark {
                egui::Color32::from_rgb(0x14, 0x16, 0x1a)
            } else {
                egui::Color32::from_rgb(0xff, 0xff, 0xff)
            })
            .rounding(6.0)
            .inner_margin(egui::Margin::same(8.0))
            .show(ui, |ui| {
                ui.set_min_height(110.0);
                if abas.len() >= 2 {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 12.0;
                        if aba(ui, tr!("Todas", "All"), self.aba_log.is_none(), None, dark) {
                            self.aba_log = None;
                        }
                        for (i, nome) in abas.iter().enumerate() {
                            let ativa = self.aba_log.as_deref() == Some(nome.as_str());
                            if aba(ui, nome, ativa, Some(cor_da_conta(i, dark)), dark) {
                                self.aba_log = Some(nome.clone());
                            }
                        }
                    });
                    ui.add_space(4.0);
                }
                egui::ScrollArea::vertical()
                    // each tab keeps its own scroll position
                    .id_salt(("log", self.aba_log.clone()))
                    .max_height(120.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        if entradas.is_empty() {
                            ui.label(
                                egui::RichText::new(tr!(
                                    "As mensagens das conexões aparecem aqui.",
                                    "Connection messages appear here."
                                ))
                                    .small()
                                    .color(label_color(dark)),
                            );
                        }
                        // same size and font as the log always had (small text)
                        let fonte = egui::FontId::proportional(12.0);
                        let normal = ui.visuals().text_color();
                        for e in &entradas {
                            let mut linha = egui::text::LayoutJob::default();
                            let mut parte = |texto: &str, cor: egui::Color32| {
                                linha.append(
                                    texto,
                                    0.0,
                                    egui::TextFormat::simple(fonte.clone(), cor),
                                );
                            };
                            parte(&format!("{} ", e.hora), label_color(dark));
                            // in "All", the account name in its color; in one
                            // account's tab the name would be redundant
                            if self.aba_log.is_none() && !e.origem.is_empty() {
                                let cor = abas
                                    .iter()
                                    .position(|n| n == &e.origem)
                                    .map(|i| cor_da_conta(i, dark))
                                    .unwrap_or(normal);
                                parte(&format!("[{}] ", e.origem), cor);
                            }
                            parte(&e.msg, normal);
                            linha.wrap.max_width = ui.available_width();
                            ui.label(linha);
                        }
                    });
            });
    }

    fn abrir_editor(&mut self, conta: Conta, nova: bool) {
        self.tela = Tela::Contas;
        self.editor = Some(Editor {
            conta,
            nova,
            mostrar_seed: false,
            mostrar_senha: false,
            erro: None,
        });
    }

    fn tela_contas(&mut self, ui: &mut egui::Ui) {
        if self.editor.is_some() {
            self.tela_editor(ui);
            return;
        }
        let m = engine::get();
        let contas = m.contas();
        let fundo_cartao = fundo_cartao(self.dark);

        ui.label(egui::RichText::new(tr!("Contas", "Accounts")).size(18.0).strong());
        ui.add_space(4.0);
        let mut editar: Option<Conta> = None;
        let mut remover: Option<Conta> = None;

        egui::ScrollArea::vertical()
            .max_height((ui.available_height() - 60.0).max(120.0))
            .auto_shrink([false, true])
            .show(ui, |ui| {
                if contas.is_empty() {
                    ui.label(
                        egui::RichText::new(tr!(
                            "Nenhuma conta ainda. Adicione a primeira abaixo.",
                            "No accounts yet. Add the first one below."
                        ))
                            .color(label_color(self.dark)),
                    );
                }
                for conta in &contas {
                    let ativa = m.ativa(&conta.id);
                    egui::Frame::none()
                        .fill(fundo_cartao)
                        .rounding(8.0)
                        .inner_margin(egui::Margin::symmetric(12.0, 10.0))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                // name on the buttons row, without invading their space
                                let largura_nome = (ui.available_width() - 200.0).max(80.0);
                                ui.vertical(|ui| {
                                    ui.set_max_width(largura_nome);
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(conta.nome_exibicao()).strong(),
                                        )
                                        .truncate(),
                                    );
                                });
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let dica = tr!(
                                            "Desconecte a conta antes de alterá-la.",
                                            "Disconnect the account before changing it."
                                        );
                                        let r = ui.add_enabled(
                                            !ativa,
                                            egui::Button::new(tr!("Remover", "Remove")),
                                        );
                                        if r.clicked() {
                                            remover = Some(conta.clone());
                                        }
                                        if ativa {
                                            r.on_disabled_hover_text(dica);
                                        }
                                        let r = ui.add_enabled(
                                            !ativa,
                                            egui::Button::new(tr!("Editar", "Edit")),
                                        );
                                        if r.clicked() {
                                            editar = Some(conta.clone());
                                        }
                                        if ativa {
                                            r.on_disabled_hover_text(dica);
                                        }
                                    },
                                );
                            });
                            // file and authentication get the full width
                            let arquivo = conta.arquivo();
                            let mut detalhes = format!(
                                "{}  ·  {}",
                                if arquivo.is_empty() {
                                    tr!("sem arquivo .ovpn", "no .ovpn file")
                                } else {
                                    &arquivo
                                },
                                conta.autenticacao.rotulo()
                            );
                            if let Some((tipo, _)) = tipo_de_tunel(conta) {
                                detalhes.push_str("  ·  ");
                                detalhes.push_str(tipo);
                            }
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(detalhes)
                                    .small()
                                    .color(label_color(self.dark)),
                                )
                                .truncate(),
                            );
                        });
                    ui.add_space(6.0);
                }
            });

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let nova = egui::Button::new(
                egui::RichText::new(tr!("Nova conta", "New account")).color(egui::Color32::WHITE),
            )
            .fill(ACCENT);
            if ui.add(nova).clicked() {
                let mut c = Conta::nova();
                c.config = find_default_config()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
                ajustar_autenticacao(&mut c);
                self.abrir_editor(c, true);
            }
        });

        if let Some(c) = editar {
            self.abrir_editor(c, false);
        }
        if let Some(c) = remover {
            let sim = rfd::MessageDialog::new()
                .set_title(APP_TITLE)
                .set_description(trf!(
                    "Remover a conta \"{}\"?\n\nUsuário, seed e senha salvos dela serão apagados.",
                    "Remove the account \"{}\"?\n\nIts saved username, seed and password will be deleted.",
                    c.nome_exibicao()
                ))
                .set_level(rfd::MessageLevel::Warning)
                .set_buttons(rfd::MessageButtons::YesNo)
                .show();
            if sim == rfd::MessageDialogResult::Yes {
                m.remover_conta(&c.id);
            }
        }
    }

    /// Settings: startup, appearance and updates, in cards like the accounts.
    fn tela_configuracoes(&mut self, ui: &mut egui::Ui) {
        use i18n::Idioma;
        use update::Estado;
        let m = engine::get();
        let dark = self.dark;
        let fraco = label_color(dark);
        let pequeno = |t: &str| egui::RichText::new(t).small().color(fraco);

        ui.label(egui::RichText::new(tr!("Configurações", "Settings")).size(18.0).strong());
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // ---------------------------------------------------- startup
                secao(ui, tr!("INICIALIZAÇÃO", "STARTUP"), dark, |ui| {
                    let mut com_windows = startup::ativo();
                    if linha_toggle(
                        ui,
                        dark,
                        tr!("Iniciar com o Windows", "Start with Windows"),
                        tr!(
                            "Abre o app quando você entra no Windows.",
                            "Opens the app when you sign in to Windows."
                        ),
                        &mut com_windows,
                        true,
                    ) {
                        if let Err(e) = startup::definir(com_windows) {
                            error_box(&e);
                        }
                    }
                    ui.add_space(4.0);
                    let mut minimizado = m.iniciar_minimizado();
                    if linha_toggle(
                        ui,
                        dark,
                        tr!("Iniciar minimizado", "Start minimized"),
                        tr!(
                            "Ao abrir com o Windows, fica só no ícone da bandeja.",
                            "When opened by Windows, stays in the tray icon only."
                        ),
                        &mut minimizado,
                        com_windows,
                    ) {
                        m.salvar_iniciar_minimizado(minimizado);
                    }

                    ui.add_space(6.0);
                    ui.separator();
                    ui.label(
                        egui::RichText::new(tr!("Conectar ao abrir", "Connect on open"))
                            .color(ui.visuals().strong_text_color()),
                    );
                    ui.label(pequeno(tr!(
                        "Estas contas conectam sozinhas sempre que o app abre.",
                        "These accounts connect by themselves whenever the app opens."
                    )));
                    ui.add_space(2.0);
                    let contas = m.contas();
                    if contas.is_empty() {
                        ui.label(pequeno(tr!("Nenhuma conta cadastrada.", "No accounts yet.")));
                    }
                    for conta in contas {
                        let mut ligado = conta.conectar_ao_abrir;
                        if linha_toggle(ui, dark, conta.nome_exibicao(), "", &mut ligado, true) {
                            m.definir_conectar_ao_abrir(&conta.id, ligado);
                        }
                    }
                });

                // ------------------------------------------------- appearance
                secao(ui, tr!("APARÊNCIA", "APPEARANCE"), dark, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(tr!("Tema", "Theme")).color(ui.visuals().strong_text_color()));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            // right to left: the last one added is the leftmost
                            for (escuro, nome) in [(false, tr!("Claro", "Light")), (true, tr!("Escuro", "Dark"))] {
                                if ui.selectable_label(self.dark == escuro, nome).clicked() && self.dark != escuro {
                                    self.dark = escuro;
                                    apply_style(ui.ctx(), escuro);
                                    m.salvar_tema(escuro);
                                }
                            }
                        });
                    });
                    ui.add_space(4.0);
                    let escolhido = m.idioma();
                    let automatico = trf!("Automático ({})", "Automatic ({})", i18n::do_windows().nome());
                    let mut novo = escolhido;
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(tr!("Idioma", "Language")).color(ui.visuals().strong_text_color()));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            egui::ComboBox::from_id_salt("idioma")
                                .selected_text(escolhido.map(Idioma::nome).unwrap_or(&automatico))
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut novo, None, automatico.as_str());
                                    for i in [Idioma::Portugues, Idioma::Ingles] {
                                        ui.selectable_value(&mut novo, Some(i), i.nome());
                                    }
                                });
                        });
                    });
                    if novo != escolhido {
                        m.salvar_idioma(novo);
                        apply_tray_state();
                    }
                });

                // ---------------------------------------------------- updates
                secao(ui, tr!("ATUALIZAÇÕES", "UPDATES"), dark, |ui| {
                    let mut auto = m.verifica_atualizacoes();
                    if linha_toggle(
                        ui,
                        dark,
                        tr!("Procurar novas versões", "Check for new versions"),
                        tr!(
                            "Uma vez por dia, no GitHub, sem enviar dados seus.",
                            "Once a day, on GitHub, without sending any of your data."
                        ),
                        &mut auto,
                        true,
                    ) {
                        m.salvar_verifica_atualizacoes(auto);
                    }
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(pequeno(&trf!("Versão {}", "Version {}", update::VERSAO_ATUAL)));
                        let estado = update::estado();
                        match &estado {
                            Estado::Verificando => {
                                ui.spinner();
                                ui.label(pequeno(tr!("procurando...", "checking...")));
                            }
                            Estado::EmDia => {
                                ui.label(pequeno(tr!(
                                    "·  você já tem a versão mais recente",
                                    "·  you have the latest version"
                                )));
                            }
                            Estado::FalhaVerificacao(e) => {
                                ui.add(egui::Label::new(pequeno(&format!("·  {e}"))).truncate());
                            }
                            Estado::Nada => {}
                            _ => {
                                if let Some(v) = update::disponivel() {
                                    let texto = egui::RichText::new(trf!(
                                        "·  versão {v} disponível",
                                        "·  version {v} available"
                                    ))
                                    .small()
                                    .color(ACCENT);
                                    if ui.link(texto).clicked() {
                                        self.janela_atualizacao = true;
                                    }
                                }
                            }
                        }
                        if matches!(estado, Estado::Nada | Estado::EmDia | Estado::FalhaVerificacao(_))
                            && ui
                                .link(egui::RichText::new(tr!("Procurar agora", "Check now")).small())
                                .clicked()
                        {
                            update::verificar_agora();
                        }
                    });
                });
            });
    }

    fn janela_atualizacao(&mut self, ctx: &egui::Context) {
        use update::Estado;
        let estado = update::estado();
        let versao = match &estado {
            Estado::Disponivel(v)
            | Estado::Baixando(v, _)
            | Estado::FalhaInstalacao(v, _)
            | Estado::Reiniciando(v) => v.clone(),
            // VPN_SCREENSHOT=update opens before the check finishes
            _ => return,
        };
        let dark = self.dark;
        let ocupada = matches!(estado, Estado::Baixando(..) | Estado::Reiniciando(_));
        let mut aberta = true;
        let mut fechar = false;
        let mut instalar = false;

        let mut janela = egui::Window::new(tr!("Atualização", "Update"))
            .id(egui::Id::new("janela_atualizacao"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]);
        if !ocupada {
            janela = janela.open(&mut aberta);
        }
        janela.show(ctx, |ui| {
            ui.set_max_width(380.0);
            match &estado {
                Estado::Disponivel(v) => {
                    ui.label(
                        egui::RichText::new(trf!(
                            "A versão {} está disponível.",
                            "Version {} is available.",
                            v.numero
                        ))
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new(trf!(
                            "Você usa a versão {}.",
                            "You are using version {}.",
                            update::VERSAO_ATUAL
                        ))
                        .color(label_color(dark)),
                    );
                    ui.add_space(6.0);
                    ui.label(if v.instalavel() {
                        tr!(
                            "O aplicativo baixa a versão nova, confere a integridade e reabre \
                             sozinho. As VPNs conectadas caem por alguns segundos e voltam em \
                             seguida.",
                            "The app downloads the new version, verifies it and reopens by \
                             itself. Connected VPNs drop for a few seconds and then come back."
                        )
                    } else {
                        tr!(
                            "Esta versão precisa ser baixada pela página.",
                            "This version must be downloaded from the release page."
                        )
                    });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if v.instalavel() {
                            let btn = egui::Button::new(
                                egui::RichText::new(tr!("Atualizar agora", "Update now"))
                                    .color(egui::Color32::WHITE),
                            )
                            .fill(ACCENT);
                            if ui.add(btn).clicked() {
                                instalar = true;
                            }
                        }
                        if ui.button(tr!("Ver novidades", "What's new")).clicked() {
                            let _ = open::that(&v.pagina);
                        }
                        if ui.button(tr!("Agora não", "Not now")).clicked() {
                            fechar = true;
                        }
                    });
                }
                Estado::Baixando(v, fracao) => {
                    ui.label(trf!("Baixando a versão {}...", "Downloading version {}...", v.numero));
                    ui.add(egui::ProgressBar::new(*fracao).show_percentage());
                }
                Estado::Reiniciando(v) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(trf!(
                            "Versão {} instalada. Reabrindo o aplicativo...",
                            "Version {} installed. Reopening the app...",
                            v.numero
                        ));
                    });
                }
                Estado::FalhaInstalacao(v, erro) => {
                    ui.colored_label(egui::Color32::from_rgb(0xdc, 0x26, 0x26), erro);
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if v.instalavel() && ui.button(tr!("Tentar de novo", "Try again")).clicked() {
                            instalar = true;
                        }
                        if ui.button(tr!("Abrir página da versão", "Open release page")).clicked() {
                            let _ = open::that(&v.pagina);
                        }
                        if ui.button(tr!("Fechar", "Close")).clicked() {
                            fechar = true;
                        }
                    });
                }
                _ => {}
            }
        });
        if instalar {
            update::instalar(versao);
        }
        if fechar || !aberta {
            self.janela_atualizacao = false;
        }
    }

    fn tela_editor(&mut self, ui: &mut egui::Ui) {
        let dark = self.dark;
        let mut salvar = false;
        let mut cancelar = false;
        let mut abrir_qr = false;
        let Some(ed) = self.editor.as_mut() else { return };

        ui.label(
            egui::RichText::new(if ed.nova {
                tr!("Nova conta", "New account")
            } else {
                tr!("Editar conta", "Edit account")
            })
                .size(18.0)
                .strong(),
        );
        ui.add_space(4.0);

        egui::ScrollArea::vertical()
            .max_height((ui.available_height() - 60.0).max(120.0))
            .auto_shrink([false, true])
            .show(ui, |ui| {
                rotulo(ui, tr!("NOME", "NAME"), dark);
                ui.add(
                    egui::TextEdit::singleline(&mut ed.conta.nome)
                        .hint_text(tr!("ex.: Trabalho, Cliente X", "e.g. Work, Client X"))
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(4.0);

                rotulo(ui, tr!("ARQUIVO DE CONFIGURAÇÃO (.OVPN)", "CONFIGURATION FILE (.OVPN)"), dark);
                ui.horizontal(|ui| {
                    let mut nome_arquivo = ed.conta.arquivo();
                    if nome_arquivo.is_empty() {
                        nome_arquivo = tr!("nenhum arquivo selecionado", "no file selected").into();
                    }
                    let largura = ui.available_width() - 118.0;
                    ui.add_enabled(
                        false,
                        egui::TextEdit::singleline(&mut nome_arquivo).desired_width(largura),
                    );
                    if ui.button(tr!("Procurar...", "Browse...")).clicked() {
                        let inicio = PathBuf::from(&ed.conta.config)
                            .parent()
                            .map(PathBuf::from)
                            .filter(|p| p.is_dir())
                            .or_else(|| config_dirs().into_iter().find(|d| d.is_dir()));
                        let mut dlg = rfd::FileDialog::new()
                            .add_filter(tr!("Config OpenVPN", "OpenVPN config"), &["ovpn"])
                            .set_title(tr!("Escolha o arquivo .ovpn", "Choose the .ovpn file"));
                        if let Some(dir) = inicio {
                            dlg = dlg.set_directory(dir);
                        }
                        if let Some(p) = dlg.pick_file() {
                            ed.conta.config = p.to_string_lossy().into_owned();
                            ajustar_autenticacao(&mut ed.conta);
                            if ed.conta.nome.trim().is_empty() {
                                if let Some(stem) = p.file_stem() {
                                    ed.conta.nome = stem.to_string_lossy().into_owned();
                                }
                            }
                        }
                    }
                });
                ui.add_space(4.0);

                if ed.conta.autenticacao.usa_usuario() {
                    rotulo(ui, tr!("USUÁRIO", "USERNAME"), dark);
                    ui.add(
                        egui::TextEdit::singleline(&mut ed.conta.usuario)
                            .desired_width(f32::INFINITY),
                    );
                    ui.add_space(4.0);
                }

                rotulo(ui, tr!("AUTENTICAÇÃO", "AUTHENTICATION"), dark);
                ui.horizontal_wrapped(|ui| {
                    for a in Autenticacao::TODAS {
                        ui.radio_value(&mut ed.conta.autenticacao, a, a.rotulo());
                    }
                });
                ui.add_space(4.0);

                if ed.conta.autenticacao.usa_senha() {
                    rotulo(ui, tr!("SENHA", "PASSWORD"), dark);
                    ui.horizontal(|ui| {
                        let largura = ui.available_width() - 98.0;
                        ui.add(
                            egui::TextEdit::singleline(&mut ed.conta.senha)
                                .password(!ed.mostrar_senha)
                                .desired_width(largura),
                        );
                        ui.checkbox(&mut ed.mostrar_senha, tr!("mostrar", "show"));
                    });
                    ui.add_space(4.0);
                }

                if ed.conta.autenticacao.usa_token() {
                    ui.horizontal(|ui| {
                        rotulo(ui, tr!("SEED DO GOOGLE AUTHENTICATOR", "GOOGLE AUTHENTICATOR SEED"), dark);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .button(
                                    egui::RichText::new(tr!("Importar QR Code...", "Import QR code..."))
                                        .small(),
                                )
                                .clicked()
                            {
                                abrir_qr = true;
                            }
                        });
                    });
                    ui.horizontal(|ui| {
                        let largura = ui.available_width() - 98.0;
                        ui.add(
                            egui::TextEdit::singleline(&mut ed.conta.seed)
                                .password(!ed.mostrar_seed)
                                .desired_width(largura),
                        );
                        ui.checkbox(&mut ed.mostrar_seed, tr!("mostrar", "show"));
                    });
                    // token preview: compare with the phone before saving
                    if let Some(seed) = totp::normalize_seed(&ed.conta.seed) {
                        if let Some(token) = totp::totp_now(&seed) {
                            ui.label(
                                egui::RichText::new(trf!(
                                    "Token atual: {} {}   ·   muda em {}s",
                                    "Current token: {} {}   ·   changes in {}s",
                                    &token[..3],
                                    &token[3..],
                                    totp::seconds_remaining()
                                ))
                                .color(label_color(dark)),
                            );
                        }
                    }
                }

                if let Some(erro) = &ed.erro {
                    ui.add_space(4.0);
                    ui.colored_label(egui::Color32::from_rgb(0xdc, 0x26, 0x26), erro);
                }
            });

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let btn = egui::Button::new(
                egui::RichText::new(tr!("Salvar", "Save")).color(egui::Color32::WHITE),
            )
                .fill(ACCENT);
            if ui.add(btn).clicked() {
                salvar = true;
            }
            if ui.button(tr!("Cancelar", "Cancel")).clicked() {
                cancelar = true;
            }
        });

        if abrir_qr {
            self.qr_open = true;
        }
        if cancelar {
            self.editor = None;
            self.qr_open = false;
            return;
        }
        if salvar {
            let Some(ed) = self.editor.as_mut() else { return };
            let mut c = ed.conta.clone();
            c.nome = c.nome.trim().to_string();
            c.usuario = c.usuario.trim().to_string();
            if !c.autenticacao.usa_usuario() {
                c.usuario.clear();
            }
            if let Some(seed) = totp::normalize_seed(&c.seed) {
                c.seed = seed;
            }
            if !c.autenticacao.usa_token() {
                c.seed.clear();
            }
            if !c.autenticacao.usa_senha() {
                c.senha.clear();
            }
            match c.validar() {
                Ok(()) => {
                    let msg = if ed.nova {
                        tr!("Conta criada.", "Account created.")
                    } else {
                        tr!("Conta atualizada.", "Account updated.")
                    };
                    state::log(c.nome_exibicao(), msg);
                    engine::get().salvar_conta(c);
                    self.editor = None;
                    self.qr_open = false;
                    apply_tray_state();
                }
                Err(e) => ed.erro = Some(e),
            }
        }
    }

    fn janela_qr(&mut self, ctx: &egui::Context) {
        let mut open_flag = true;
        let mut do_file = false;
        let mut do_clip = false;
        egui::Window::new(tr!("Importar QR Code", "Import QR code"))
            .id(egui::Id::new("janela_qr"))
            .open(&mut open_flag)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(tr!(
                    "Quando o suporte cadastra seu token, você recebe um QR Code —\n\
                     o mesmo que é escaneado no app Google Authenticator do celular.\n\
                     Esse QR Code contém o usuário e a seed:\n\n\
                     1. Salve a imagem do QR Code no computador (ou copie com\n\
                        PrintScreen / Ferramenta de Captura);\n\
                     2. Use um dos botões abaixo;\n\
                     3. Confira os campos preenchidos e clique em Salvar.\n\n\
                     Se você não tem o QR Code, peça ao suporte o recadastramento.",
                    "When support enrolls your token, you receive a QR code —\n\
                     the same one scanned with the Google Authenticator phone app.\n\
                     That QR code contains the username and the seed:\n\n\
                     1. Save the QR code image on the computer (or copy it with\n\
                        PrintScreen / Snipping Tool);\n\
                     2. Use one of the buttons below;\n\
                     3. Check the filled-in fields and click Save.\n\n\
                     If you don't have the QR code, ask support to enroll you again."
                ));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let pick = egui::Button::new(
                        egui::RichText::new(tr!("Escolher imagem...", "Choose image..."))
                            .color(egui::Color32::WHITE),
                    )
                    .fill(ACCENT);
                    if ui.add(pick).clicked() {
                        do_file = true;
                    }
                    if ui.button(tr!("Colar imagem copiada", "Paste copied image")).clicked() {
                        do_clip = true;
                    }
                });
            });
        self.qr_open = open_flag;
        if do_file {
            self.qr_from_file();
        }
        if do_clip {
            self.qr_from_clipboard();
        }
    }
}

/// Global egui context for the threads (filled in when the App is created).
static EGUI_CTX: std::sync::OnceLock<egui::Context> = std::sync::OnceLock::new();
fn eframe_ctx() -> egui::Context {
    EGUI_CTX.get().cloned().unwrap_or_default()
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        install_main_window_hook();
        if MAIN_WNDPROC_INSTALLED.load(Ordering::SeqCst) && INICIAR_OCULTO.swap(false, Ordering::SeqCst) {
            unsafe {
                let hwnd = find_main_window();
                if !hwnd.is_null() {
                    esconder_janela(hwnd);
                }
            }
        }
        self.poll_events();
        apply_tray_state();
        if ABRIR_ATUALIZACAO.swap(false, Ordering::SeqCst) {
            self.janela_atualizacao = true;
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::central_panel(&ctx.style())
                    .inner_margin(egui::Margin::symmetric(18.0, 14.0)),
            )
            .show(ctx, |ui| {
                // ---------- header ----------
                ui.horizontal(|ui| {
                    ui.heading(egui::RichText::new(APP_TITLE).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // each screen's button turns into "Back" in the same spot:
                        // open and close a screen without moving the mouse
                        let voltar = |ui: &mut egui::Ui| {
                            ui.add(egui::Button::new(tr!("Voltar", "Back")).min_size(egui::vec2(0.0, 32.0)))
                                .clicked()
                        };
                        // rightmost slot: settings (or "Back" while in settings)
                        match self.tela {
                            Tela::Configuracoes => {
                                if voltar(ui) {
                                    self.tela = Tela::Inicio;
                                    update::limpar_resultado();
                                }
                            }
                            Tela::Inicio => {
                                let b = egui::Button::new(egui::RichText::new("⚙").size(18.0))
                                    .min_size(egui::vec2(40.0, 32.0));
                                if ui.add(b).on_hover_text(tr!("Configurações", "Settings")).clicked() {
                                    self.tela = Tela::Configuracoes;
                                }
                            }
                            Tela::Contas => {
                                // same gear, so "Back" stays exactly where "Accounts"
                                // was; disabled while an account is being edited
                                let b = egui::Button::new(egui::RichText::new("⚙").size(18.0))
                                    .min_size(egui::vec2(40.0, 32.0));
                                if ui
                                    .add_enabled(self.editor.is_none(), b)
                                    .on_hover_text(tr!("Configurações", "Settings"))
                                    .clicked()
                                {
                                    self.tela = Tela::Configuracoes;
                                }
                            }
                        }
                        if self.tela == Tela::Inicio {
                            let b = egui::Button::new(tr!("Contas", "Accounts"))
                                .min_size(egui::vec2(0.0, 32.0));
                            let dica = tr!("Cadastrar e editar contas", "Add and edit accounts");
                            if ui.add(b).on_hover_text(dica).clicked() {
                                self.tela = Tela::Contas;
                            }
                        } else if self.tela == Tela::Contas && self.editor.is_none() && voltar(ui) {
                            self.tela = Tela::Inicio;
                        }
                        // new version: just an unobtrusive link in the header
                        if let Some(v) = update::disponivel() {
                            let texto = egui::RichText::new(trf!(
                                "Versão {v} disponível",
                                "Version {v} available"
                            ))
                                .small()
                                .color(ACCENT);
                            if ui
                                .link(texto)
                                .on_hover_text(tr!("Ver e instalar a atualização", "View and install the update"))
                                .clicked()
                            {
                                self.janela_atualizacao = true;
                            }
                        }
                    });
                });
                ui.add_space(8.0);

                self.banners(ui);

                match self.tela {
                    Tela::Inicio => self.tela_inicio(ui),
                    Tela::Contas => self.tela_contas(ui),
                    Tela::Configuracoes => self.tela_configuracoes(ui),
                }
            });

        if self.qr_open {
            self.janela_qr(ctx);
        }
        if self.janela_atualizacao {
            self.janela_atualizacao(ctx);
        }

        if MAIN_WINDOW_VISIBLE.load(Ordering::SeqCst) {
            // the token preview changes every second
            let intervalo = if self.editor.is_some() {
                Duration::from_secs(1)
            } else {
                OPENVPN_CHECK_INTERVAL
                    .saturating_sub(self.last_ovpn_check.elapsed())
                    .max(Duration::from_millis(100))
            };
            ctx.request_repaint_after(intervalo);
        }
    }
}

fn main() -> eframe::Result<()> {
    // elevated helper (started by service::autorizar_usuario_atual, after the
    // UAC prompt): authorizes the user in the OpenVPN service and exits
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == service::ARG_AUTHORIZE) {
        let usuario = args.get(i + 1).cloned().unwrap_or_default();
        std::process::exit(service::adicionar_ao_grupo(&usuario));
    }

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

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(WINDOW_SIZE)
            .with_resizable(false)
            .with_icon(std::sync::Arc::new(icon))
            .with_title(WINDOW_TITLE),
        // DO NOT switch to OpenGL/glow: in virtual machines and remote desktop
        // sessions Windows only offers software OpenGL 1.1 and the app does not
        // even open. DX12 always finds an adapter - when there is no real GPU it
        // falls back to WARP (Microsoft Basic Render Driver), which supports
        // DirectX 12. That switch is what made the app open in VMs.
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
            supported_backends: eframe::wgpu::Backends::DX12,
            // static interface: no reason to wake up the dedicated GPU
            // (saves battery and memory on hybrid laptops)
            power_preference: eframe::wgpu::PowerPreference::LowPower,
            ..Default::default()
        },
        ..Default::default()
    };

    let result = eframe::run_native(
        WINDOW_TITLE,
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    );

    if let Err(error) = &result {
        report_startup_error(error);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatacao_de_bytes() {
        assert_eq!(fmt_bytes(0.0), "0 B");
        assert_eq!(fmt_bytes(512.0), "512 B");
        assert_eq!(fmt_bytes(1536.0), "1,5 KB");
        assert_eq!(fmt_bytes(1024.0 * 1024.0 * 2.3), "2,3 MB");
        assert_eq!(fmt_rate(1024.0 * 120.0), "120,0 KB/s");
    }

    /// Every symbol shown in the UI MUST exist in egui's fonts, otherwise it
    /// becomes a "box". This test locks the approved list.
    #[test]
    fn glifos_presentes_nas_fontes_do_egui() {
        let defs = egui::FontDefinitions::default();
        // what matters is the Proportional family (used in the UI texts)
        let proportional = &defs.families[&egui::FontFamily::Proportional];
        let covered = |c: char| {
            proportional.iter().any(|name| {
                defs.font_data.get(name).is_some_and(|data| {
                    ttf_parser::Face::parse(&data.font, 0)
                        .map(|f| f.glyph_index(c).is_some())
                        .unwrap_or(false)
                })
            })
        };
        // symbols actually used in the UI:
        for c in ['\u{2022}', '·', '\u{2B07}', '\u{2B06}', '—', '⚙'] {
            assert!(covered(c), "egui proportional font lacks {c:?}");
        }
    }

    #[test]
    fn mapeamento_de_estado_do_openvpn() {
        assert_eq!(state::situacao_do_openvpn("CONNECTED"), Situacao::Conectado);
        assert_eq!(state::situacao_do_openvpn("RECONNECTING"), Situacao::Reconectando);
        assert_eq!(state::situacao_do_openvpn("EXITING"), Situacao::Desconectando);
        assert_eq!(state::situacao_do_openvpn("WAIT"), Situacao::Conectando);
    }

    /// Regression (v1.0.2): tooltips wider than the window were cut at its edge.
    #[test]
    fn tooltips_fit_inside_the_window() {
        let ctx = egui::Context::default();
        for dark in [true, false] {
            apply_style(&ctx, dark);
            let largura = ctx.style().spacing.tooltip_width;
            // room for the tooltip frame and the window border
            assert!(largura + 40.0 <= WINDOW_SIZE[0], "tooltip width {largura} does not fit");
        }
    }

    #[test]
    fn menu_da_bandeja_tem_um_item_por_conta() {
        let contas = vec![
            ("a".to_string(), "Trabalho".to_string()),
            ("b".to_string(), "Cliente X".to_string()),
        ];
        let (_menu, itens, acoes) = montar_menu(&contas, None);
        assert_eq!(itens.len(), 2);
        let alternar: Vec<String> = acoes
            .iter()
            .filter_map(|(_, a)| match a {
                AcaoMenu::Alternar(id) => Some(id.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(alternar, vec!["a", "b"]);
        assert!(acoes.iter().any(|(_, a)| matches!(a, AcaoMenu::DesconectarTodas)));
        assert!(acoes.iter().any(|(_, a)| matches!(a, AcaoMenu::Sair)));

        assert!(!acoes.iter().any(|(_, a)| matches!(a, AcaoMenu::Atualizar)));

        // a single account: no "Disconnect all"
        let (_m, _i, acoes) = montar_menu(&contas[..1], None);
        assert!(!acoes.iter().any(|(_, a)| matches!(a, AcaoMenu::DesconectarTodas)));

        // new version published: item to update
        let (_m, _i, acoes) = montar_menu(&contas, Some("1.0.1"));
        assert!(acoes.iter().any(|(_, a)| matches!(a, AcaoMenu::Atualizar)));
    }
}
