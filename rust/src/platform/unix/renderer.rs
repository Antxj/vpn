//! Linux and macOS: the window is always drawn with OpenGL (Linux: Mesa or
//! the video driver, and Mesa draws on the CPU when there is no GPU; macOS:
//! the system's OpenGL), so there is no fallback to choose. Same interface as
//! the Windows module.

pub const ARG_DX12: &str = "--dx12";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Desenho {
    OpenGl,
}

impl Desenho {
    pub fn nome(self) -> &'static str {
        "OpenGL"
    }

    pub fn eframe(self) -> eframe::Renderer {
        eframe::Renderer::Glow
    }

    /// Never the exception that goes to the log.
    pub fn excecao(self) -> bool {
        false
    }
}

pub fn escolher(_forcar_dx12: bool) -> (Desenho, &'static str) {
    (Desenho::OpenGl, "")
}

pub fn vigiar_inicio_opengl() {}

pub fn opengl_funcionou() {}

pub fn falhou_ao_abrir() -> bool {
    false
}

/// Options of the main window. Started minimized, the window is created hidden.
pub fn opcoes(
    desenho: Desenho,
    viewport: eframe::egui::ViewportBuilder,
    oculto: bool,
) -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: viewport.with_visible(!oculto),
        renderer: desenho.eframe(),
        ..Default::default()
    }
}
