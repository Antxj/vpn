//! Native dialogs (messages, yes/no questions, open/save file). The
//! implementation of each system is in platform/<os>/dialogs.rs (Windows:
//! rfd; Linux: GTK on the tray's GTK thread).

use crate::platform::dialogs as imp;
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
