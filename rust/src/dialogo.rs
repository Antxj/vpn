//! Native dialogs (messages, yes/no questions, open/save file).
//!
//! Windows: rfd. Linux: GTK, run on the same GTK thread as the tray icon
//! (GTK only works from the thread that started it; two GTK threads would
//! crash) - see linux/desktop.rs.

use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nivel {
    Info,
    Aviso,
    Erro,
}

/// File type filter: a name and the extensions (without the dot).
#[derive(Clone, Copy)]
pub struct Filtro<'a> {
    pub nome: &'a str,
    pub extensoes: &'a [&'a str],
}

pub fn mensagem(texto: &str, nivel: Nivel) {
    imp::mensagem(texto, nivel)
}

/// True when the user answered "Yes".
pub fn sim_nao(texto: &str, nivel: Nivel) -> bool {
    imp::sim_nao(texto, nivel)
}

pub fn abrir_arquivo(titulo: &str, filtro: Filtro, pasta: Option<PathBuf>) -> Option<PathBuf> {
    imp::abrir_arquivo(titulo, filtro, pasta)
}

/// `nome`: suggested file name (with the extension).
pub fn salvar_arquivo(titulo: &str, filtro: Filtro, nome: &str) -> Option<PathBuf> {
    imp::salvar_arquivo(titulo, filtro, nome)
}

#[cfg(windows)]
mod imp {
    use super::{Filtro, Nivel};
    use std::path::PathBuf;

    fn nivel(n: Nivel) -> rfd::MessageLevel {
        match n {
            Nivel::Info => rfd::MessageLevel::Info,
            Nivel::Aviso => rfd::MessageLevel::Warning,
            Nivel::Erro => rfd::MessageLevel::Error,
        }
    }

    pub fn mensagem(texto: &str, n: Nivel) {
        rfd::MessageDialog::new()
            .set_title(crate::APP_TITLE)
            .set_description(texto)
            .set_level(nivel(n))
            .show();
    }

    pub fn sim_nao(texto: &str, n: Nivel) -> bool {
        rfd::MessageDialog::new()
            .set_title(crate::APP_TITLE)
            .set_description(texto)
            .set_level(nivel(n))
            .set_buttons(rfd::MessageButtons::YesNo)
            .show()
            == rfd::MessageDialogResult::Yes
    }

    pub fn abrir_arquivo(titulo: &str, filtro: Filtro, pasta: Option<PathBuf>) -> Option<PathBuf> {
        let mut dlg = rfd::FileDialog::new()
            .add_filter(filtro.nome, filtro.extensoes)
            .set_title(titulo);
        if let Some(p) = pasta {
            dlg = dlg.set_directory(p);
        }
        dlg.pick_file()
    }

    pub fn salvar_arquivo(titulo: &str, filtro: Filtro, nome: &str) -> Option<PathBuf> {
        rfd::FileDialog::new()
            .add_filter(filtro.nome, filtro.extensoes)
            .set_title(titulo)
            .set_file_name(nome)
            .save_file()
    }
}

#[cfg(target_os = "linux")]
mod imp {
    pub use crate::desktop::dialogos::*;
}
