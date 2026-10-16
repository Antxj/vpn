//! Windows and macOS: native dialogs through rfd (on macOS rfd runs them on
//! the main thread by itself, as AppKit requires).

use crate::dialogs::{Filtro, Nivel};
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
