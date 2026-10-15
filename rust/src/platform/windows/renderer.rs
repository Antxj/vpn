//! Which graphics API draws the window.
//!
//! OpenGL is the default: for a small window like this one it is lighter and
//! faster than DirectX 12 (measured: ~84 MB instead of ~113 MB with the
//! window open, 107 MB instead of 449 MB committed, a third less time to
//! open). But remote desktop sessions, virtual machines and PCs without a
//! video driver only have Windows' software OpenGL 1.1, where the window
//! does not open at all. There DirectX 12 is used: it always works (when
//! there is no real GPU, Windows draws it on the CPU with WARP).

use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};

/// Internal argument: start with DirectX 12 (after OpenGL failed to open the
/// window), waiting for the process with the PID that follows to exit.
pub const ARG_DX12: &str = "--dx12";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Desenho {
    OpenGl,
    DirectX12,
}

impl Desenho {
    pub fn nome(self) -> &'static str {
        match self {
            Desenho::OpenGl => "OpenGL",
            Desenho::DirectX12 => "DirectX 12",
        }
    }

    pub fn eframe(self) -> eframe::Renderer {
        match self {
            Desenho::OpenGl => eframe::Renderer::Glow,
            Desenho::DirectX12 => eframe::Renderer::Wgpu,
        }
    }

    /// DirectX 12 is the exception, worth a line in the log.
    pub fn excecao(self) -> bool {
        self == Desenho::DirectX12
    }
}

/// Marker left on this PC when OpenGL failed to open the window: from then
/// on this version goes straight to DirectX 12 here. It holds the app version,
/// so the next version tries OpenGL again (a new video driver may fix it).
fn marcador() -> PathBuf {
    crate::settings::app_dir().join("opengl-falhou")
}

/// While OpenGL has not drawn its first frames yet. If the app stops in that
/// window (an error or an abort inside the graphics library), the next start
/// uses DirectX 12.
static INICIANDO_OPENGL: AtomicBool = AtomicBool::new(false);

/// Call right before opening the window with OpenGL. Installs a hook that,
/// if the graphics library aborts the app while starting, marks this PC and
/// opens the app again with DirectX 12 (the abort cannot be caught).
pub fn vigiar_inicio_opengl() {
    INICIANDO_OPENGL.store(true, Ordering::SeqCst);
    let anterior = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if INICIANDO_OPENGL.swap(false, Ordering::SeqCst) {
            reabrir_com_dx12();
        }
        anterior(info);
    }));
}

/// The first OpenGL frames were drawn: no more fallback from here on.
pub fn opengl_funcionou() {
    INICIANDO_OPENGL.store(false, Ordering::SeqCst);
}

/// OpenGL failed while opening the window: marks this PC and starts the app
/// again with DirectX 12 (same arguments). True if the new process started.
pub fn falhou_ao_abrir() -> bool {
    INICIANDO_OPENGL.swap(false, Ordering::SeqCst) && reabrir_com_dx12()
}

fn reabrir_com_dx12() -> bool {
    let _ = std::fs::create_dir_all(crate::settings::app_dir());
    let _ = std::fs::write(marcador(), crate::update::VERSAO_ATUAL);
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    args.push(ARG_DX12.to_string());
    args.push(std::process::id().to_string());
    std::process::Command::new(exe).args(args).spawn().is_ok()
}

/// The choice, and why (for the log).
pub fn escolher(forcar_dx12: bool) -> (Desenho, &'static str) {
    if forcar_dx12 {
        return (
            Desenho::DirectX12,
            tr!("o OpenGL não abriu a janela", "OpenGL could not open the window"),
        );
    }
    let falhou = std::fs::read_to_string(marcador()).is_ok_and(|v| v.trim() == crate::update::VERSAO_ATUAL);
    if falhou {
        return (
            Desenho::DirectX12,
            tr!("o OpenGL já falhou neste computador", "OpenGL already failed on this computer"),
        );
    }
    // VPN_RENDERER=opengl|dx12 forces one (tests)
    match std::env::var("VPN_RENDERER").as_deref() {
        Ok("opengl") => return (Desenho::OpenGl, "VPN_RENDERER"),
        Ok("dx12") => return (Desenho::DirectX12, "VPN_RENDERER"),
        _ => {}
    }
    if sessao_remota() {
        return (Desenho::DirectX12, tr!("sessão de área de trabalho remota", "remote desktop session"));
    }
    match versao_do_opengl() {
        Some((versao, placa)) if opengl_suficiente(&versao, &placa) => (Desenho::OpenGl, ""),
        _ => (
            Desenho::DirectX12,
            tr!("sem OpenGL do driver de vídeo", "no OpenGL from the video driver"),
        ),
    }
}

fn sessao_remota() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_REMOTESESSION};
    unsafe { GetSystemMetrics(SM_REMOTESESSION) != 0 }
}

/// OpenGL 2.0 or newer from a real driver. "GDI Generic" is Windows' own
/// software OpenGL 1.1, which egui cannot use.
fn opengl_suficiente(versao: &str, placa: &str) -> bool {
    if placa.contains("GDI Generic") {
        return false;
    }
    let mut numeros = versao
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<u32>().unwrap_or(0));
    let maior = numeros.next().unwrap_or(0);
    maior >= 2
}

/// (GL_VERSION, GL_RENDERER) of the OpenGL this PC offers, read through a
/// throwaway hidden window (a few milliseconds). None if it cannot be created.
fn versao_do_opengl() -> Option<(String, String)> {
    use windows_sys::Win32::Graphics::Gdi::{GetDC, ReleaseDC};
    use windows_sys::Win32::Graphics::OpenGL::{
        glGetString, wglCreateContext, wglDeleteContext, wglMakeCurrent, ChoosePixelFormat,
        SetPixelFormat, GL_RENDERER, GL_VERSION, PFD_DOUBLEBUFFER, PFD_DRAW_TO_WINDOW,
        PFD_MAIN_PLANE, PFD_SUPPORT_OPENGL, PFD_TYPE_RGBA, PIXELFORMATDESCRIPTOR,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WS_POPUP};

    unsafe fn texto(nome: u32) -> String {
        let p = glGetString(nome);
        if p.is_null() {
            return String::new();
        }
        std::ffi::CStr::from_ptr(p as *const std::ffi::c_char)
            .to_string_lossy()
            .into_owned()
    }

    unsafe {
        // a plain STATIC window of our own, only to get a device context
        let classe = super::single::wide("STATIC");
        let janela = CreateWindowExW(
            0,
            classe.as_ptr(),
            ptr::null(),
            WS_POPUP,
            0,
            0,
            1,
            1,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null::<c_void>(),
        );
        if janela.is_null() {
            return None;
        }
        let dc = GetDC(janela);
        let mut resultado = None;
        if !dc.is_null() {
            let mut pfd: PIXELFORMATDESCRIPTOR = std::mem::zeroed();
            pfd.nSize = std::mem::size_of::<PIXELFORMATDESCRIPTOR>() as u16;
            pfd.nVersion = 1;
            pfd.dwFlags = PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL | PFD_DOUBLEBUFFER;
            pfd.iPixelType = PFD_TYPE_RGBA;
            pfd.cColorBits = 32;
            pfd.iLayerType = PFD_MAIN_PLANE as u8;
            let formato = ChoosePixelFormat(dc, &pfd);
            if formato != 0 && SetPixelFormat(dc, formato, &pfd) != 0 {
                let contexto = wglCreateContext(dc);
                if !contexto.is_null() {
                    if wglMakeCurrent(dc, contexto) != 0 {
                        resultado = Some((texto(GL_VERSION), texto(GL_RENDERER)));
                        wglMakeCurrent(ptr::null_mut(), ptr::null_mut());
                    }
                    wglDeleteContext(contexto);
                }
            }
            ReleaseDC(janela, dc);
        }
        DestroyWindow(janela);
        resultado
    }
}

/// Options of the main window: OpenGL where the video driver offers it;
/// DirectX 12 in remote desktop sessions, VMs and PCs with only Windows'
/// software OpenGL 1.1. `_oculto` (started minimized): eframe ignores a
/// hidden window on Windows; desktop.rs cloaks it instead.
pub fn opcoes(desenho: Desenho, viewport: eframe::egui::ViewportBuilder, _oculto: bool) -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport,
        renderer: desenho.eframe(),
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
            supported_backends: eframe::wgpu::Backends::DX12,
            // static interface: no reason to wake up the dedicated GPU
            // (saves battery and memory on hybrid laptops)
            power_preference: eframe::wgpu::PowerPreference::LowPower,
            ..Default::default()
        },
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn so_opengl_de_driver_a_partir_do_2() {
        assert!(opengl_suficiente("4.6.0 NVIDIA 560.94", "NVIDIA GeForce RTX 3060/PCIe/SSE2"));
        assert!(opengl_suficiente("3.3.0 - Build 31.0.101.2111", "Intel(R) UHD Graphics 620"));
        assert!(opengl_suficiente("2.1", "Some driver"));
        // Windows' software OpenGL (remote desktop, VMs, no video driver)
        assert!(!opengl_suficiente("1.1.0", "GDI Generic"));
        assert!(!opengl_suficiente("1.4", "Old driver"));
        assert!(!opengl_suficiente("", ""));
    }

    #[test]
    fn opengl_deste_pc_e_lido() {
        // any Windows has OpenGL 1.1 at least: the probe itself must work
        let (versao, placa) = versao_do_opengl().expect("probe failed");
        assert!(!versao.is_empty() && !placa.is_empty(), "{versao:?} {placa:?}");
    }
}
