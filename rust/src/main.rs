//! VPN - conexoes OpenVPN (uma ou varias ao mesmo tempo) com token TOTP
//! gerado automaticamente.
#![windows_subsystem = "windows"]

mod atualizacao;
mod contas;
mod dpapi;
mod estado;
mod installer;
mod motor;
mod qr;
mod single;
mod totp;
mod vpn;

use contas::{Autenticacao, Conta};
use eframe::egui;
use estado::{Agregado, Situacao};
use motor::ErroConexao;
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
const OPENVPN_CHECK_INTERVAL: Duration = Duration::from_secs(5);

static MAIN_WINDOW_VISIBLE: AtomicBool = AtomicBool::new(true);
static TRAY_HINT_SHOWN: AtomicBool = AtomicBool::new(false);
static MAIN_WNDPROC_INSTALLED: AtomicBool = AtomicBool::new(false);
static ORIGINAL_MAIN_WNDPROC: AtomicIsize = AtomicIsize::new(0);
static ORIGINAL_MAIN_EXSTYLE: AtomicIsize = AtomicIsize::new(0);
/// Pedido do menu da bandeja para abrir a janela de atualizacao.
static ABRIR_ATUALIZACAO: AtomicBool = AtomicBool::new(false);
/// Esta abertura veio de uma atualizacao feita pelo proprio app.
static APOS_ATUALIZAR: AtomicBool = AtomicBool::new(false);
/// Contas que estavam ligadas antes da atualizacao (religadas ao abrir).
static RECONECTAR: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();

const OPENVPN_CANDIDATES: &[&str] = &[
    r"C:\Program Files\OpenVPN\bin\openvpn.exe",
    r"C:\Program Files (x86)\OpenVPN\bin\openvpn.exe",
];
/// Chave que o instalador do OpenVPN grava (pega instalacao fora do
/// caminho padrao, por exemplo em outro disco).
const OPENVPN_REG_KEY: &str = r"SOFTWARE\OpenVPN";

// ------------------------------------------------------- Win32 (janela) ---

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

/// Esconde/mostra a janela via DWM (cloaking). Retorna false se o DWM
/// recusou - nesse caso quem chama precisa esconder do jeito classico,
/// senao a janela continuaria visivel.
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
    // Classe interna da crate tray-icon (pinada pelo Cargo.lock). Se um dia
    // ela mudar, a notificacao de balao para de aparecer em silencio - o
    // resto da bandeja continua funcionando.
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
            "O aplicativo VPN continua ativo na bandeja. Clique no ícone para reabrir.",
        );
        return Shell_NotifyIconW(NIM_MODIFY, &notification) != 0;
    }

    false
}

/// Janela principal deste processo. Procura pelo titulo E pelo processo:
/// com um nome generico como "VPN", outro programa poderia ter uma janela
/// com o mesmo titulo.
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

/// Restaura a janela principal direto pela API do Windows.
/// Funciona mesmo com o loop do egui pausado (janela oculta).
pub fn show_main_window() {
    MAIN_WINDOW_VISIBLE.store(true, Ordering::SeqCst);
    unsafe {
        let hwnd = find_main_window();
        if !hwnd.is_null() {
            set_main_window_taskbar(hwnd, true);
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

unsafe extern "system" fn main_wnd_proc(
    hwnd: *mut c_void,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    if msg == WM_CLOSE || (msg == WM_SYSCOMMAND && wparam & 0xFFF0 == SC_MINIMIZE) {
        let cloaked = set_main_window_cloaked(hwnd, true);
        set_main_window_taskbar(hwnd, false);
        if !cloaked {
            // DWM indisponivel (raro): set_main_window_taskbar termina com
            // SW_SHOW, entao sem esta linha a janela reapareceria
            ShowWindow(hwnd, SW_HIDE);
        }
        MAIN_WINDOW_VISIBLE.store(false, Ordering::SeqCst);
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

// ------------------------------------------------------------ utilidades ---

/// Le HKLM\SOFTWARE\OpenVPN\exe_path (gravado pelo instalador oficial).
fn openvpn_from_registry() -> Option<PathBuf> {
    use windows_sys::Win32::System::Registry::{
        RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6432KEY,
        RRF_SUBKEY_WOW6464KEY,
    };

    let key = single::wide(OPENVPN_REG_KEY);
    let value = single::wide("exe_path");
    // tenta a visao de 64 e a de 32 bits do registro
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
    // VPN_OPENVPN permite apontar um openvpn.exe fora do caminho
    // padrao (e, com um caminho inexistente, testar o aviso de ausencia)
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
    let img = image::load_from_memory(bytes).expect("icone embutido invalido");
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    (rgba.into_raw(), w, h)
}

/// Cor dos rotulos de campo (mais legivel que o "weak" padrao do egui).
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
        Ok(()) => format!("Detalhes registrados em:\n{}", log_path.display()),
        Err(log_error) => format!("Nao foi possivel gravar o log: {log_error}"),
    };
    error_box(&format!(
        "Nao foi possivel iniciar a interface grafica.\n\nErro: {error}\n\n{log_info}"
    ));
}

/// "1234567" bytes -> "1,2 MB" (virgula pt-BR).
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
        format!("{v:.1} {}", UNITS[unit]).replace('.', ",")
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
    ctx.set_style(style);
}

// ------------------------------- bandeja (so na thread principal/Win32) ---

#[derive(Clone)]
enum AcaoMenu {
    Abrir,
    Atualizar,
    Alternar(String),
    DesconectarTodas,
    Sair,
}

/// Acao de cada item do menu da bandeja. O menu e refeito quando as contas
/// mudam; o handler (que roda fora do loop do egui) consulta esta tabela.
static MENU_ACOES: Mutex<Vec<(MenuId, AcaoMenu)>> = Mutex::new(Vec::new());

struct TrayUi {
    tray: TrayIcon,
    icons: [tray_icon::Icon; 3], // cinza, ambar, verde
    /// Item marcavel de cada conta, para refletir conectada/desconectada.
    itens: Vec<(String, CheckMenuItem)>,
    /// (id, nome) das contas e versao nova oferecida no menu atual;
    /// None = menu ainda nao montado.
    assinatura: Option<(Vec<(String, String)>, Option<String>)>,
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

    let abrir = MenuItem::new("Abrir", true, None);
    acoes.push((abrir.id().clone(), AcaoMenu::Abrir));
    let _ = menu.append(&abrir);
    if let Some(v) = versao_nova {
        let item = MenuItem::new(format!("Atualizar para a versão {v}..."), true, None);
        acoes.push((item.id().clone(), AcaoMenu::Atualizar));
        let _ = menu.append(&item);
    }
    let _ = menu.append(&PredefinedMenuItem::separator());

    if contas.is_empty() {
        let _ = menu.append(&MenuItem::new("Nenhuma conta cadastrada", false, None));
    }
    for (id, nome) in contas {
        let item = CheckMenuItem::new(nome, true, false, None);
        acoes.push((item.id().clone(), AcaoMenu::Alternar(id.clone())));
        let _ = menu.append(&item);
        itens.push((id.clone(), item));
    }
    if contas.len() > 1 {
        let todas = MenuItem::new("Desconectar todas", true, None);
        acoes.push((todas.id().clone(), AcaoMenu::DesconectarTodas));
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&todas);
    }

    let _ = menu.append(&PredefinedMenuItem::separator());
    let sair = MenuItem::new("Sair", true, None);
    acoes.push((sair.id().clone(), AcaoMenu::Sair));
    let _ = menu.append(&sair);
    (menu, itens, acoes)
}

/// Conta "ligada" para efeito de toggle/marcacao: conexao em andamento que
/// nao esta sendo encerrada.
fn conta_ligada(id: &str) -> bool {
    motor::get().ativa(id) && estado::obter(id).situacao != Situacao::Desconectando
}

/// Atualiza icone, tooltip e menu da bandeja a partir do estado compartilhado.
/// Roda na thread principal: chamada pelo App e pelo timer Win32 (que
/// funciona com a janela oculta).
fn apply_tray_state() {
    let nomes = motor::get().nomes();
    let versao_nova = atualizacao::disponivel();
    TRAY_UI.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let Some(ui) = borrow.as_mut() else { return };

        let assinatura = (nomes.clone(), versao_nova.clone());
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

        let (agregado, linhas) = estado::resumo(&nomes);
        let icon_idx = match agregado {
            Agregado::Conectado => 2,
            Agregado::Transicao => 1,
            Agregado::Nenhuma => 0,
        };
        // limite do Windows para o tooltip da bandeja: 127 caracteres
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

/// Janela invisivel com timer de 1s: mantem a bandeja (icone/tooltip/menu)
/// atualizada mesmo quando o loop do egui esta pausado (janela oculta).
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

/// Liga ou desliga uma conta. Usada pela interface e pelo menu da bandeja.
/// Pede confirmacao quando duas conexoes disputariam a rota padrao.
fn alternar_conta(id: &str) -> Result<(), ErroConexao> {
    let m = motor::get();
    if m.ativa(id) {
        m.desconectar(id);
        return Ok(());
    }
    let conflitos = m.conflitos_de_rota(id);
    if !conflitos.is_empty() {
        let nome = m.conta(id).map(|c| c.nome_exibicao().to_string()).unwrap_or_default();
        let resposta = rfd::MessageDialog::new()
            .set_title(APP_TITLE)
            .set_description(format!(
                "\"{nome}\" e \"{}\" mandam todo o tráfego da internet pela VPN.\n\n\
                 Com as duas conectadas, só a última funciona como rota padrão \
                 (a outra continua acessando apenas a própria rede). Conectar mesmo assim?",
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
                    // sem a janela nao da para oferecer a instalacao: abre-a,
                    // onde o aviso amarelo tem o botao "Instalar agora"
                    show_main_window();
                    error_box(&e.mensagem());
                }
            }
            Some(AcaoMenu::DesconectarTodas) => motor::get().desconectar_todas(),
            Some(AcaoMenu::Sair) => {
                // Sair com garantia: desconecta (ate 15s) e encerra o
                // processo, sem depender do loop do egui estar acordado.
                std::thread::spawn(|| motor::get().encerrar());
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

/// Mostra um erro sem travar quem chamou (threads de conexao).
pub fn error_box_async(msg: String) {
    if cfg!(test) {
        estado::log("", msg); // nada de janelas modais durante os testes
        return;
    }
    std::thread::spawn(move || error_box(&msg));
}

/// Toggle (interruptor) desenhado: ligado = azul.
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
        let fundo = if habilitado { fundo } else { fundo.gamma_multiply(0.5) };
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

fn cor_situacao(s: Situacao, padrao: egui::Color32) -> egui::Color32 {
    match s {
        Situacao::Conectado => egui::Color32::from_rgb(0x2a, 0xa0, 0x2a),
        Situacao::Conectando | Situacao::Reconectando | Situacao::Desconectando => {
            egui::Color32::from_rgb(0xb5, 0x89, 0x00)
        }
        Situacao::Desconectado => padrao,
    }
}

fn rotulo(ui: &mut egui::Ui, texto: &str, dark: bool) {
    ui.label(egui::RichText::new(texto).small().color(label_color(dark)));
}

// -------------------------------------------------------------------- app ---

#[derive(PartialEq)]
enum Tela {
    Inicio,
    Contas,
}

/// Conta sendo criada ou editada na tela de contas.
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
    janela_atualizacao: bool,
    admin: bool,
    openvpn_missing: bool,
    last_ovpn_check: Instant,
    installing: bool,
    install_rx: Receiver<installer::Event>,
    install_tx: Sender<installer::Event>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let m = motor::get();
        let dark = m.tema_escuro();
        apply_style(&cc.egui_ctx, dark);
        let _ = EGUI_CTX.set(cc.egui_ctx.clone());
        let ctx = cc.egui_ctx.clone();
        estado::ao_mudar(move || ctx.request_repaint());

        single::spawn_show_listener();

        // ---- bandeja ----
        let (g, gw, gh) = load_icon_rgba(include_bytes!("../assets/gray_32.png"));
        let (w_, ww, wh) = load_icon_rgba(include_bytes!("../assets/warn_32.png"));
        let (o, ow, oh) = load_icon_rgba(include_bytes!("../assets/ok_32.png"));
        let icon_gray = tray_icon::Icon::from_rgba(g, gw, gh).unwrap();
        let icon_warn = tray_icon::Icon::from_rgba(w_, ww, wh).unwrap();
        let icon_ok = tray_icon::Icon::from_rgba(o, ow, oh).unwrap();

        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(Menu::new()))
            .with_menu_on_left_click(false)
            .with_tooltip(format!("{APP_TITLE} - Desconectado"))
            .with_icon(icon_gray.clone())
            .build()
            .expect("falha ao criar o icone da bandeja");
        install_tray_handlers();

        TRAY_UI.with(|cell| {
            *cell.borrow_mut() = Some(TrayUi {
                tray,
                icons: [icon_gray, icon_warn, icon_ok],
                itens: Vec::new(),
                assinatura: None,
                last_icon: 0,
                last_tip: format!("{APP_TITLE} - Desconectado"),
            });
        });
        apply_tray_state();
        create_tick_window();

        let (install_tx, install_rx) = std::sync::mpsc::channel();
        // VPN_CAPTURA: so para as capturas de tela da documentacao, feitas com
        // o build de desenvolvimento (que roda sem elevacao de proposito)
        let admin = motor::eh_administrador() || std::env::var_os("VPN_CAPTURA").is_some();
        if !admin {
            estado::log("", "Atenção: o aplicativo não está como administrador.");
        }
        if APOS_ATUALIZAR.load(Ordering::SeqCst) {
            estado::log(
                "",
                format!("Aplicativo atualizado para a versão {}.", atualizacao::VERSAO_ATUAL),
            );
        }
        // religa as contas que estavam conectadas antes da atualizacao
        for id in RECONECTAR.get().into_iter().flatten() {
            if let Err(e) = m.conectar(id, find_openvpn()) {
                let nome = m.conta(id).map(|c| c.nome_exibicao().to_string()).unwrap_or_default();
                estado::log(&nome, format!("Não reconectou: {}", e.mensagem()));
            }
        }
        let ctx = cc.egui_ctx.clone();
        atualizacao::ao_mudar(move || ctx.request_repaint());
        atualizacao::iniciar_verificacao_periodica(|| motor::get().verifica_atualizacoes());

        let mut app = Self {
            dark,
            // sem contas, abre direto no cadastro
            tela: if m.contas().is_empty() { Tela::Contas } else { Tela::Inicio },
            editor: None,
            qr_open: false,
            janela_atualizacao: false,
            admin,
            openvpn_missing: find_openvpn().is_none(),
            last_ovpn_check: Instant::now(),
            installing: false,
            install_rx,
            install_tx,
        };
        // VPN_CAPTURA=contas|editar|nova abre direto naquela tela (capturas
        // de tela da documentacao e conferencia visual de cada tela)
        match std::env::var("VPN_CAPTURA").as_deref() {
            Ok("contas") => app.tela = Tela::Contas,
            Ok("editar") => {
                if let Some(c) = m.contas().into_iter().next() {
                    app.abrir_editor(c, false);
                }
            }
            Ok("nova") => app.abrir_editor(Conta::nova(), true),
            Ok("atualizacao") => app.janela_atualizacao = true,
            _ => {}
        }
        app
    }

    /// Instala o OpenVPN embutido (silencioso, em segundo plano).
    fn start_openvpn_install(&mut self) {
        if self.installing || !installer::is_available() {
            return;
        }
        self.installing = true;
        estado::log(
            "",
            format!(
                "Instalando o OpenVPN Community {}... (pode levar cerca de um minuto)",
                installer::MSI_VERSION
            ),
        );
        installer::install_in_background(self.install_tx.clone(), eframe_ctx());
    }

    /// Mostra o erro de conexao; quando falta o OpenVPN, oferece instalar.
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
                format!(
                    "O OpenVPN Community não está instalado — ele é necessário \
                     para conectar à VPN.\n\nInstalar agora? O aplicativo já traz \
                     o instalador oficial (versão {}) e faz tudo sozinho, sem \
                     precisar baixar nada.",
                    installer::MSI_VERSION
                ),
                true,
            )
        } else {
            (
                "O OpenVPN Community não está instalado — ele é necessário \
                 para conectar à VPN.\n\nAbrir a página de download agora?"
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
                            estado::log("", "Instalação concluída, mas o OpenVPN não foi encontrado.");
                            show_main_window();
                            error_box(
                                "A instalação terminou, mas o OpenVPN não foi encontrado.\n\
                                 Reinicie o computador e abra o aplicativo de novo.",
                            );
                        }
                        Ok(reiniciar) => {
                            estado::log("", "OpenVPN Community instalado com sucesso.");
                            if reiniciar {
                                estado::log(
                                    "",
                                    "O Windows pediu reinicialização; se a conexão falhar, reinicie.",
                                );
                            }
                        }
                        Err(msg) => {
                            estado::log("", format!("Falha na instalação: {msg}"));
                            show_main_window();
                            error_box(&format!("Não foi possível instalar o OpenVPN.\n\n{msg}"));
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

    // ------------------------------------------------------------ QR code --

    fn import_qr_image(&mut self, img: image::DynamicImage) {
        let Some(text) = qr::decode_qr(&img) else {
            error_box(
                "Não encontrei um QR Code nessa imagem.\n\
                 Confira se ele aparece inteiro e nítido.",
            );
            return;
        };
        let data = qr::parse_payload(&text);
        let Some(seed) = data.seed else {
            let preview: String = text.chars().take(200).collect();
            error_box(&format!(
                "Li o QR Code, mas não identifiquei uma seed nele.\n\nConteúdo:\n{preview}"
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
            .add_filter("Imagens", &["png", "jpg", "jpeg", "bmp", "gif", "webp"])
            .set_title("Escolha a imagem do QR Code")
            .pick_file()
        else {
            return;
        };
        match image::open(&path) {
            Ok(img) => self.import_qr_image(img),
            Err(_) => error_box("Não consegui abrir essa imagem."),
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
                    .set_description(
                        "Não há imagem na área de transferência.\n\
                         Copie a imagem do QR Code e tente de novo.",
                    )
                    .set_level(rfd::MessageLevel::Info)
                    .show();
            }
        }
    }

    // ------------------------------------------------------------- telas --

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

        if !self.admin {
            faixa(
                ui,
                "O aplicativo não está como administrador.\n\
                 Feche e abra de novo aceitando a permissão do Windows.",
                &mut |_| {},
            );
        }

        if self.openvpn_missing {
            let embutido = installer::is_available();
            let texto = if self.installing {
                "Instalando o OpenVPN Community...\nIsso leva cerca de um minuto."
            } else {
                "OpenVPN Community não está instalado.\nEle é necessário para conectar à VPN."
            };
            let mut instalar = false;
            let instalando = self.installing;
            faixa(ui, texto, &mut |ui| {
                if instalando {
                    ui.spinner();
                } else {
                    let rotulo = if embutido { "Instalar agora" } else { "Baixar" };
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
        let m = motor::get();
        let contas = m.contas();

        if contas.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("Nenhuma conta cadastrada.").size(16.0));
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("Cadastre a primeira VPN para conectar.")
                        .color(label_color(self.dark)),
                );
                ui.add_space(12.0);
                let btn = egui::Button::new(
                    egui::RichText::new("Adicionar conta").color(egui::Color32::WHITE),
                )
                .fill(ACCENT);
                if ui.add(btn).clicked() {
                    self.abrir_editor(Conta::nova(), true);
                }
            });
            return;
        }

        let fundo_cartao = if self.dark {
            egui::Color32::from_rgb(0x26, 0x29, 0x31)
        } else {
            egui::Color32::from_rgb(0xff, 0xff, 0xff)
        };
        // a lista ocupa o espaco de cima e o log fica ancorado no rodape
        let varias_ativas = contas.iter().filter(|c| m.ativa(&c.id)).count() > 1;
        let altura_log = 150.0 + if varias_ativas { 44.0 } else { 0.0 };
        let altura_lista = (ui.available_height() - altura_log).max(120.0);
        let mut erro: Option<ErroConexao> = None;

        egui::ScrollArea::vertical()
            .max_height(altura_lista)
            .min_scrolled_height(altura_lista)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for conta in &contas {
                    let e = estado::obter(&conta.id);
                    let ligada = conta_ligada(&conta.id);
                    egui::Frame::none()
                        .fill(fundo_cartao)
                        .rounding(8.0)
                        .inner_margin(egui::Margin::symmetric(12.0, 10.0))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                // o texto nunca invade o espaco do interruptor:
                                // o que nao couber termina em reticencias
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
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(format!("\u{2022}  {}", e.texto()))
                                                .color(cor),
                                        )
                                        .truncate(),
                                    );
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
                                                egui::RichText::new(format!(
                                                    "recebido {}  ·  enviado {}",
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
                                            if ligada { "Desconectar" } else { "Conectar" },
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
                if ui.button("Desconectar todas").clicked() {
                    m.desconectar_todas();
                }
            });
        }
        ui.add_space(4.0);
        self.painel_log(ui);
    }

    fn painel_log(&self, ui: &mut egui::Ui) {
        egui::Frame::none()
            .fill(if self.dark {
                egui::Color32::from_rgb(0x14, 0x16, 0x1a)
            } else {
                egui::Color32::from_rgb(0xff, 0xff, 0xff)
            })
            .rounding(6.0)
            .inner_margin(egui::Margin::same(8.0))
            .show(ui, |ui| {
                ui.set_min_height(110.0);
                egui::ScrollArea::vertical()
                    .id_salt("log")
                    .max_height(120.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let linhas = estado::log_linhas();
                        if linhas.is_empty() {
                            ui.label(
                                egui::RichText::new("As mensagens das conexões aparecem aqui.")
                                    .small()
                                    .color(label_color(self.dark)),
                            );
                        }
                        for line in linhas {
                            ui.label(egui::RichText::new(line).monospace().small());
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
        let m = motor::get();
        let contas = m.contas();
        let fundo_cartao = if self.dark {
            egui::Color32::from_rgb(0x26, 0x29, 0x31)
        } else {
            egui::Color32::from_rgb(0xff, 0xff, 0xff)
        };

        ui.label(egui::RichText::new("Contas").size(18.0).strong());
        ui.add_space(4.0);
        let mut editar: Option<Conta> = None;
        let mut remover: Option<Conta> = None;

        egui::ScrollArea::vertical()
            .max_height((ui.available_height() - 120.0).max(120.0))
            .auto_shrink([false, true])
            .show(ui, |ui| {
                if contas.is_empty() {
                    ui.label(
                        egui::RichText::new("Nenhuma conta ainda. Adicione a primeira abaixo.")
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
                                // nome na linha dos botoes, sem invadir o espaco deles
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
                                        let dica = "Desconecte a conta antes de alterá-la.";
                                        let r = ui.add_enabled(!ativa, egui::Button::new("Remover"));
                                        if r.clicked() {
                                            remover = Some(conta.clone());
                                        }
                                        if ativa {
                                            r.on_disabled_hover_text(dica);
                                        }
                                        let r = ui.add_enabled(!ativa, egui::Button::new("Editar"));
                                        if r.clicked() {
                                            editar = Some(conta.clone());
                                        }
                                        if ativa {
                                            r.on_disabled_hover_text(dica);
                                        }
                                    },
                                );
                            });
                            // arquivo e autenticacao ganham a largura toda
                            let arquivo = conta.arquivo();
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(format!(
                                        "{}  ·  {}",
                                        if arquivo.is_empty() { "sem arquivo .ovpn" } else { &arquivo },
                                        conta.autenticacao.rotulo()
                                    ))
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
                egui::RichText::new("Nova conta").color(egui::Color32::WHITE),
            )
            .fill(ACCENT);
            if ui.add(nova).clicked() {
                let mut c = Conta::nova();
                c.config = find_default_config()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.abrir_editor(c, true);
            }
            if ui.button("Voltar").clicked() {
                self.tela = Tela::Inicio;
                atualizacao::limpar_resultado();
            }
        });
        self.rodape_atualizacao(ui);

        if let Some(c) = editar {
            self.abrir_editor(c, false);
        }
        if let Some(c) = remover {
            let sim = rfd::MessageDialog::new()
                .set_title(APP_TITLE)
                .set_description(format!(
                    "Remover a conta \"{}\"?\n\nUsuário, seed e senha salvos dela serão apagados.",
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

    /// Versao e atualizacoes: discreto, no rodape da tela de contas.
    fn rodape_atualizacao(&mut self, ui: &mut egui::Ui) {
        use atualizacao::Estado;
        let m = motor::get();
        let fraco = label_color(self.dark);
        let pequeno = |t: &str| egui::RichText::new(t).small().color(fraco);
        ui.add_space(10.0);
        ui.separator();
        let mut auto = m.verifica_atualizacoes();
        if ui
            .checkbox(&mut auto, pequeno("Procurar novas versões automaticamente"))
            .on_hover_text("Uma vez por dia o aplicativo consulta o GitHub, sem enviar dados seus.")
            .changed()
        {
            m.salvar_verifica_atualizacoes(auto);
        }
        ui.horizontal(|ui| {
            ui.label(pequeno(&format!("Versão {}", atualizacao::VERSAO_ATUAL)));
            let estado = atualizacao::estado();
            match &estado {
                Estado::Verificando => {
                    ui.spinner();
                    ui.label(pequeno("procurando..."));
                }
                Estado::EmDia => {
                    ui.label(pequeno("·  você já tem a versão mais recente"));
                }
                Estado::FalhaVerificacao(e) => {
                    ui.add(egui::Label::new(pequeno(&format!("·  {e}"))).truncate());
                }
                Estado::Nada => {}
                _ => {
                    if let Some(v) = atualizacao::disponivel() {
                        let texto = egui::RichText::new(format!("·  versão {v} disponível"))
                            .small()
                            .color(ACCENT);
                        if ui.link(texto).clicked() {
                            self.janela_atualizacao = true;
                        }
                    }
                }
            }
            if matches!(estado, Estado::Nada | Estado::EmDia | Estado::FalhaVerificacao(_))
                && ui.link(egui::RichText::new("Procurar agora").small()).clicked()
            {
                atualizacao::verificar_agora();
            }
        });
    }

    fn janela_atualizacao(&mut self, ctx: &egui::Context) {
        use atualizacao::Estado;
        let estado = atualizacao::estado();
        let versao = match &estado {
            Estado::Disponivel(v)
            | Estado::Baixando(v, _)
            | Estado::FalhaInstalacao(v, _)
            | Estado::Reiniciando(v) => v.clone(),
            // VPN_CAPTURA=atualizacao abre antes de a verificacao terminar
            _ => return,
        };
        let dark = self.dark;
        let ocupada = matches!(estado, Estado::Baixando(..) | Estado::Reiniciando(_));
        let mut aberta = true;
        let mut fechar = false;
        let mut instalar = false;

        let mut janela = egui::Window::new("Atualização")
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
                        egui::RichText::new(format!("A versão {} está disponível.", v.numero))
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new(format!(
                            "Você usa a versão {}.",
                            atualizacao::VERSAO_ATUAL
                        ))
                        .color(label_color(dark)),
                    );
                    ui.add_space(6.0);
                    ui.label(if v.instalavel() {
                        "O aplicativo baixa a versão nova, confere a integridade e reabre \
                         sozinho. As VPNs conectadas caem por alguns segundos e voltam em \
                         seguida."
                    } else {
                        "Esta versão precisa ser baixada pela página."
                    });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if v.instalavel() {
                            let btn = egui::Button::new(
                                egui::RichText::new("Atualizar agora").color(egui::Color32::WHITE),
                            )
                            .fill(ACCENT);
                            if ui.add(btn).clicked() {
                                instalar = true;
                            }
                        }
                        if ui.button("Ver novidades").clicked() {
                            let _ = open::that(&v.pagina);
                        }
                        if ui.button("Agora não").clicked() {
                            fechar = true;
                        }
                    });
                }
                Estado::Baixando(v, fracao) => {
                    ui.label(format!("Baixando a versão {}...", v.numero));
                    ui.add(egui::ProgressBar::new(*fracao).show_percentage());
                }
                Estado::Reiniciando(v) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!(
                            "Versão {} instalada. Reabrindo o aplicativo...",
                            v.numero
                        ));
                    });
                }
                Estado::FalhaInstalacao(v, erro) => {
                    ui.colored_label(egui::Color32::from_rgb(0xdc, 0x26, 0x26), erro);
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if v.instalavel() && ui.button("Tentar de novo").clicked() {
                            instalar = true;
                        }
                        if ui.button("Abrir página da versão").clicked() {
                            let _ = open::that(&v.pagina);
                        }
                        if ui.button("Fechar").clicked() {
                            fechar = true;
                        }
                    });
                }
                _ => {}
            }
        });
        if instalar {
            atualizacao::instalar(versao);
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
            egui::RichText::new(if ed.nova { "Nova conta" } else { "Editar conta" })
                .size(18.0)
                .strong(),
        );
        ui.add_space(4.0);

        egui::ScrollArea::vertical()
            .max_height((ui.available_height() - 60.0).max(120.0))
            .auto_shrink([false, true])
            .show(ui, |ui| {
                rotulo(ui, "NOME", dark);
                ui.add(
                    egui::TextEdit::singleline(&mut ed.conta.nome)
                        .hint_text("ex.: Trabalho, Cliente X")
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(4.0);

                rotulo(ui, "ARQUIVO DE CONFIGURAÇÃO (.OVPN)", dark);
                ui.horizontal(|ui| {
                    let mut nome_arquivo = ed.conta.arquivo();
                    if nome_arquivo.is_empty() {
                        nome_arquivo = "nenhum arquivo selecionado".into();
                    }
                    let largura = ui.available_width() - 118.0;
                    ui.add_enabled(
                        false,
                        egui::TextEdit::singleline(&mut nome_arquivo).desired_width(largura),
                    );
                    if ui.button("Procurar...").clicked() {
                        let inicio = PathBuf::from(&ed.conta.config)
                            .parent()
                            .map(PathBuf::from)
                            .filter(|p| p.is_dir())
                            .or_else(|| config_dirs().into_iter().find(|d| d.is_dir()));
                        let mut dlg = rfd::FileDialog::new()
                            .add_filter("Config OpenVPN", &["ovpn"])
                            .set_title("Escolha o arquivo .ovpn");
                        if let Some(dir) = inicio {
                            dlg = dlg.set_directory(dir);
                        }
                        if let Some(p) = dlg.pick_file() {
                            ed.conta.config = p.to_string_lossy().into_owned();
                            if ed.conta.nome.trim().is_empty() {
                                if let Some(stem) = p.file_stem() {
                                    ed.conta.nome = stem.to_string_lossy().into_owned();
                                }
                            }
                        }
                    }
                });
                ui.add_space(4.0);

                rotulo(ui, "USUÁRIO", dark);
                ui.add(
                    egui::TextEdit::singleline(&mut ed.conta.usuario)
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(4.0);

                rotulo(ui, "AUTENTICAÇÃO", dark);
                ui.horizontal_wrapped(|ui| {
                    for a in Autenticacao::TODAS {
                        ui.radio_value(&mut ed.conta.autenticacao, a, a.rotulo());
                    }
                });
                ui.add_space(4.0);

                if ed.conta.autenticacao.usa_senha() {
                    rotulo(ui, "SENHA", dark);
                    ui.horizontal(|ui| {
                        let largura = ui.available_width() - 98.0;
                        ui.add(
                            egui::TextEdit::singleline(&mut ed.conta.senha)
                                .password(!ed.mostrar_senha)
                                .desired_width(largura),
                        );
                        ui.checkbox(&mut ed.mostrar_senha, "mostrar");
                    });
                    ui.add_space(4.0);
                }

                if ed.conta.autenticacao.usa_token() {
                    ui.horizontal(|ui| {
                        rotulo(ui, "SEED DO GOOGLE AUTHENTICATOR", dark);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .button(egui::RichText::new("Importar QR Code...").small())
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
                        ui.checkbox(&mut ed.mostrar_seed, "mostrar");
                    });
                    // previa do token: confere com o celular antes de salvar
                    if let Some(seed) = totp::normalize_seed(&ed.conta.seed) {
                        if let Some(token) = totp::totp_now(&seed) {
                            ui.label(
                                egui::RichText::new(format!(
                                    "Token atual: {} {}   ·   muda em {}s",
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
            let btn = egui::Button::new(egui::RichText::new("Salvar").color(egui::Color32::WHITE))
                .fill(ACCENT);
            if ui.add(btn).clicked() {
                salvar = true;
            }
            if ui.button("Cancelar").clicked() {
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
                    estado::log(c.nome_exibicao(), if ed.nova { "Conta criada." } else { "Conta atualizada." });
                    motor::get().salvar_conta(c);
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
        egui::Window::new("Importar QR Code")
            .open(&mut open_flag)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(
                    "Quando o suporte cadastra seu token, você recebe um QR Code —\n\
                     o mesmo que é escaneado no app Google Authenticator do celular.\n\
                     Esse QR Code contém o usuário e a seed:\n\n\
                     1. Salve a imagem do QR Code no computador (ou copie com\n\
                        PrintScreen / Ferramenta de Captura);\n\
                     2. Use um dos botões abaixo;\n\
                     3. Confira os campos preenchidos e clique em Salvar.\n\n\
                     Se você não tem o QR Code, peça ao suporte o recadastramento.",
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let pick = egui::Button::new(
                        egui::RichText::new("Escolher imagem...").color(egui::Color32::WHITE),
                    )
                    .fill(ACCENT);
                    if ui.add(pick).clicked() {
                        do_file = true;
                    }
                    if ui.button("Colar imagem copiada").clicked() {
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

/// Contexto global do egui para as threads (preenchido na criacao do App).
static EGUI_CTX: std::sync::OnceLock<egui::Context> = std::sync::OnceLock::new();
fn eframe_ctx() -> egui::Context {
    EGUI_CTX.get().cloned().unwrap_or_default()
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        install_main_window_hook();
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
                // ---------- cabecalho ----------
                ui.horizontal(|ui| {
                    ui.heading(egui::RichText::new(APP_TITLE).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // botao de tema: mostra o tema para o qual vai trocar
                        let icon = if self.dark { "☀" } else { "🌙" };
                        let btn = egui::Button::new(egui::RichText::new(icon).size(18.0))
                            .min_size(egui::vec2(40.0, 32.0));
                        if ui
                            .add(btn)
                            .on_hover_text(if self.dark { "Tema claro" } else { "Tema escuro" })
                            .clicked()
                        {
                            self.dark = !self.dark;
                            apply_style(ctx, self.dark);
                            motor::get().salvar_tema(self.dark);
                        }
                        if self.tela == Tela::Inicio {
                            let b = egui::Button::new("Contas").min_size(egui::vec2(0.0, 32.0));
                            if ui.add(b).on_hover_text("Cadastrar e editar contas").clicked() {
                                self.tela = Tela::Contas;
                            }
                        }
                        // versao nova: so um link discreto no cabecalho
                        if let Some(v) = atualizacao::disponivel() {
                            let texto = egui::RichText::new(format!("Versão {v} disponível"))
                                .small()
                                .color(ACCENT);
                            if ui
                                .link(texto)
                                .on_hover_text("Ver e instalar a atualização")
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
                }
            });

        if self.qr_open {
            self.janela_qr(ctx);
        }
        if self.janela_atualizacao {
            self.janela_atualizacao(ctx);
        }

        if MAIN_WINDOW_VISIBLE.load(Ordering::SeqCst) {
            // a previa do token muda a cada segundo
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
    // depois de "Atualizar agora", a versao anterior abre esta passando o
    // proprio PID e as contas que estavam ligadas
    let args: Vec<String> = std::env::args().collect();
    let valor = |nome: &str| {
        args.iter()
            .position(|a| a == nome)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    if let Some(pid) = valor(atualizacao::ARG_APOS_ATUALIZAR).and_then(|p| p.parse().ok()) {
        // a anterior ainda desconecta as VPNs (ate 15 s) e segura a instancia
        atualizacao::aguardar_processo(pid, Duration::from_secs(30));
        APOS_ATUALIZAR.store(true, Ordering::SeqCst);
    }
    if let Some(ids) = valor(atualizacao::ARG_RECONECTAR) {
        let _ = RECONECTAR.set(
            ids.split(',')
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect(),
        );
    }

    if !single::acquire_or_signal() {
        return Ok(()); // outra instancia ja esta rodando e foi avisada
    }
    atualizacao::limpar_restos();

    let (rgba, w, h) = load_icon_rgba(include_bytes!("../assets/app_64.png"));
    let icon = egui::IconData {
        rgba,
        width: w,
        height: h,
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([480.0, 660.0])
            .with_resizable(false)
            .with_icon(std::sync::Arc::new(icon))
            .with_title(WINDOW_TITLE),
        // NAO troque para OpenGL/glow: em maquinas virtuais e sessoes de
        // area de trabalho remota o Windows so oferece OpenGL 1.1 por
        // software e o app nem abre. O DX12 sempre encontra adaptador -
        // quando nao ha GPU real, cai no WARP (Microsoft Basic Render
        // Driver), que e compativel com DirectX 12. Foi essa troca que
        // fez o aplicativo abrir nas VMs.
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
            supported_backends: eframe::wgpu::Backends::DX12,
            // interface estatica: nao ha motivo para acordar a GPU dedicada
            // (economiza bateria e memoria em notebooks hibridos)
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

    /// Todo simbolo mostrado na interface PRECISA existir nas fontes do
    /// egui, senao vira "caixinha". Este teste trava a lista aprovada.
    #[test]
    fn glifos_presentes_nas_fontes_do_egui() {
        let defs = egui::FontDefinitions::default();
        // o que importa e a familia Proporcional (usada nos textos da UI)
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
        // simbolos efetivamente usados na interface:
        for c in ['\u{2022}', '·', '☀', '🌙', '\u{2B07}', '\u{2B06}', '—'] {
            assert!(covered(c), "fonte proporcional do egui nao tem {c:?}");
        }
    }

    #[test]
    fn mapeamento_de_estado_do_openvpn() {
        assert_eq!(estado::situacao_do_openvpn("CONNECTED"), Situacao::Conectado);
        assert_eq!(estado::situacao_do_openvpn("RECONNECTING"), Situacao::Reconectando);
        assert_eq!(estado::situacao_do_openvpn("EXITING"), Situacao::Desconectando);
        assert_eq!(estado::situacao_do_openvpn("WAIT"), Situacao::Conectando);
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

        // uma conta so: sem "Desconectar todas"
        let (_m, _i, acoes) = montar_menu(&contas[..1], None);
        assert!(!acoes.iter().any(|(_, a)| matches!(a, AcaoMenu::DesconectarTodas)));

        // versao nova publicada: item para atualizar
        let (_m, _i, acoes) = montar_menu(&contas, Some("1.0.1"));
        assert!(acoes.iter().any(|(_, a)| matches!(a, AcaoMenu::Atualizar)));
    }
}
