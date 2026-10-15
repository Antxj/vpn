//! Linux: native dialogs with GTK, run on the GTK thread of the tray icon
//! (see desktop.rs: two GTK threads would crash).

use crate::dialogs::{Filtro, Nivel};
use gtk::prelude::*;
use std::path::PathBuf;

fn caixa(texto: &str, nivel: Nivel, pergunta: bool) -> bool {
    let tipo = match nivel {
        Nivel::Info => gtk::MessageType::Info,
        Nivel::Aviso => gtk::MessageType::Warning,
        Nivel::Erro => gtk::MessageType::Error,
    };
    let botoes = if pergunta {
        gtk::ButtonsType::YesNo
    } else {
        gtk::ButtonsType::Ok
    };
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
    super::desktop::na_thread_gtk(move || {
        caixa(&texto, nivel, false);
    });
}

pub fn sim_nao(texto: &str, nivel: Nivel) -> bool {
    let texto = texto.to_string();
    super::desktop::na_thread_gtk(move || caixa(&texto, nivel, true)).unwrap_or(false)
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
    super::desktop::na_thread_gtk(move || escolher(&titulo, &nome, &extensoes, pasta, None))
        .flatten()
}

pub fn salvar_arquivo(titulo: &str, filtro: Filtro, sugerido: &str) -> Option<PathBuf> {
    let (titulo, nome, sugerido) = (
        titulo.to_string(),
        filtro.nome.to_string(),
        sugerido.to_string(),
    );
    let extensoes: Vec<String> = filtro.extensoes.iter().map(|e| e.to_string()).collect();
    super::desktop::na_thread_gtk(move || {
        escolher(&titulo, &nome, &extensoes, None, Some(sugerido))
    })
    .flatten()
}
