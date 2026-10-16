//! Windows: main window and tray integration through the Win32 API
//! (hide to the tray, cloaking, notifications, the 1 s tray timer).

use crate::*;
use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
use std::time::Duration;

/// The last notification shown was about a new version: clicking it opens
/// the update window (other notifications just open the app).
static AVISO_E_DE_VERSAO: AtomicBool = AtomicBool::new(false);
static MAIN_WNDPROC_INSTALLED: AtomicBool = AtomicBool::new(false);
static ORIGINAL_MAIN_WNDPROC: AtomicIsize = AtomicIsize::new(0);
static ORIGINAL_MAIN_EXSTYLE: AtomicIsize = AtomicIsize::new(0);
/// Original window procedure of the tray icon's hidden window (0 = not hooked).
static ORIGINAL_TRAY_WNDPROC: AtomicIsize = AtomicIsize::new(0);
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconGetRect, Shell_NotifyIconW, NIF_INFO, NIIF_INFO, NIIF_RESPECT_QUIET_TIME,
    NIIF_WARNING, NIM_MODIFY, NOTIFYICONDATAW, NOTIFYICONIDENTIFIER,
};

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
    fn GetCurrentProcess() -> *mut c_void;
    fn SetProcessWorkingSetSize(process: *mut c_void, minimum: usize, maximum: usize) -> i32;
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

/// Windows notification coming from the tray icon. `alerta` = warning icon.
/// Respects "do not disturb" (quiet time / focus assist).
unsafe fn show_tray_notification(texto: &str, alerta: bool) -> bool {
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
        notification.dwInfoFlags =
            if alerta { NIIF_WARNING } else { NIIF_INFO } | NIIF_RESPECT_QUIET_TIME;
        notification.Anonymous.uTimeout = 4_000;
        copy_notification_text(&mut notification.szInfoTitle, APP_TITLE);
        copy_notification_text(&mut notification.szInfo, texto);
        AVISO_E_DE_VERSAO.store(false, Ordering::SeqCst);
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
    devolver_memoria_quando_escondido();
}

/// Each time the window is hidden (to tell an old request from a new one).
static VEZES_ESCONDIDA: AtomicU64 = AtomicU64::new(0);

/// A few seconds after going to the tray, gives the memory the window was
/// using back to Windows (the Task Manager figure drops from ~110 MB to a few
/// MB). The pages come back by themselves when the window is opened again.
fn devolver_memoria_quando_escondido() {
    let vez = VEZES_ESCONDIDA.fetch_add(1, Ordering::SeqCst) + 1;
    std::thread::spawn(move || {
        // lets the last frame finish
        std::thread::sleep(Duration::from_secs(3));
        if VEZES_ESCONDIDA.load(Ordering::SeqCst) == vez
            && !MAIN_WINDOW_VISIBLE.load(Ordering::SeqCst)
        {
            // (-1, -1): removes as many pages as possible from the working set
            unsafe {
                SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
            }
        }
    });
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
            && show_tray_notification(
                tr!(
                    "O aplicativo VPN continua ativo na bandeja. Clique no ícone para reabrir.",
                    "VPN is still running in the system tray. Click the icon to reopen it."
                ),
                false,
            )
        {
            TRAY_HINT_SHOWN.store(true, Ordering::SeqCst);
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

/// Callback message of the tray-icon crate (pinned by Cargo.lock, like the
/// window class above). If it ever changes, clicking a notification just
/// stops opening the app - the rest of the tray keeps working.
const WM_USER_TRAYICON: u32 = 6002;
/// The user clicked the notification balloon (lParam of the callback).
const NIN_BALLOONUSERCLICK: isize = 0x0405;

unsafe extern "system" fn tray_wnd_proc(
    hwnd: *mut c_void,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    if msg == WM_USER_TRAYICON && lparam == NIN_BALLOONUSERCLICK {
        if AVISO_E_DE_VERSAO.swap(false, Ordering::SeqCst) {
            ABRIR_ATUALIZACAO.store(true, Ordering::SeqCst);
        }
        show_main_window();
        return 0;
    }
    let previous = ORIGINAL_TRAY_WNDPROC.load(Ordering::SeqCst);
    CallWindowProcW(previous as *const c_void, hwnd, msg, wparam, lparam)
}

/// Clicking one of the app's notifications opens the window (and, for a new
/// version, the update window). The tray-icon crate ignores that click.
fn install_tray_click_hook() {
    if ORIGINAL_TRAY_WNDPROC.load(Ordering::SeqCst) != 0 {
        return;
    }
    unsafe {
        let hwnd = find_own_tray_window();
        if !hwnd.is_null() {
            let previous = SetWindowLongPtrW(
                hwnd,
                GWLP_WNDPROC,
                tray_wnd_proc as *const () as usize as isize,
            );
            ORIGINAL_TRAY_WNDPROC.store(previous, Ordering::SeqCst);
        }
    }
}

/// Windows notification when the automatic check finds a new version.
pub(crate) fn notificar_nova_versao(numero: &str) {
    if cfg!(test) {
        return;
    }
    let texto = trf!(
        "Versão {numero} disponível. Clique aqui para atualizar.",
        "Version {numero} is available. Click here to update."
    );
    unsafe {
        if show_tray_notification(&texto, false) {
            AVISO_E_DE_VERSAO.store(true, Ordering::SeqCst);
        }
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

/// Windows notification from the tray icon (safe wrapper).
pub fn notificacao(texto: &str, alerta: bool) -> bool {
    unsafe { show_tray_notification(texto, alerta) }
}

/// Creates the tray on this (main) thread and the hidden timer window that
/// keeps it up to date while the egui loop is paused.
pub fn iniciar_bandeja() {
    criar_bandeja();
    apply_tray_state();
    create_tick_window();
}

/// Started minimized: cloaked before the first paint, so the window never
/// flashes; the rest of the hiding happens on the first frame.
pub fn ocultar_antes_do_primeiro_quadro() {
    if INICIAR_OCULTO.load(Ordering::SeqCst) {
        unsafe {
            let hwnd = find_main_window();
            if !hwnd.is_null() {
                set_main_window_cloaked(hwnd, true);
            }
        }
    }
}

/// Every frame: installs the window hooks (once) and hides the window when
/// started minimized.
pub fn por_quadro(_ctx: &egui::Context) {
    install_main_window_hook();
    install_tray_click_hook();
    if MAIN_WNDPROC_INSTALLED.load(Ordering::SeqCst) && INICIAR_OCULTO.swap(false, Ordering::SeqCst)
    {
        unsafe {
            let hwnd = find_main_window();
            if !hwnd.is_null() {
                esconder_janela(hwnd);
            }
        }
    }
}
