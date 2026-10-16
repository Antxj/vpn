//! Tray icon and its menu: built from the accounts and kept up to date every
//! frame (connection state, new version).

use super::*;
use crate::engine::ErroConexao;
use crate::state::{Agregado, Situacao};
use std::cell::RefCell;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

#[derive(Clone)]
pub(crate) enum AcaoMenu {
    Abrir,
    Atualizar,
    Alternar(String),
    DesconectarTodas,
    Sair,
}

/// Action of each tray menu item. The menu is rebuilt when the accounts
/// change; the handler (which runs outside the egui loop) looks up this table.
pub(crate) static MENU_ACOES: Mutex<Vec<(MenuId, AcaoMenu)>> = Mutex::new(Vec::new());

pub(crate) struct TrayUi {
    tray: TrayIcon,
    icons: [tray_icon::Icon; 3], // gray, amber, green
    /// Checkable item of each account, to reflect connected/disconnected.
    itens: Vec<(String, CheckMenuItem)>,
    /// (id, name) of the accounts, new version offered and language of the
    /// current menu; None = menu not built yet.
    assinatura: Option<AssinaturaDoMenu>,
    last_icon: usize,
    last_tip: String,
}

/// (id, name) of the accounts, new version offered, Portuguese?
pub(crate) type AssinaturaDoMenu = (Vec<(String, String)>, Option<String>, bool);

/// Tray menu, checkable item of each account, action of each menu id.
pub(crate) type MenuMontado = (Menu, Vec<(String, CheckMenuItem)>, Vec<(MenuId, AcaoMenu)>);

thread_local! {
    static TRAY_UI: RefCell<Option<TrayUi>> = const { RefCell::new(None) };
}

pub(crate) fn montar_menu(contas: &[(String, String)], versao_nova: Option<&str>) -> MenuMontado {
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
pub(crate) fn conta_ligada(id: &str) -> bool {
    engine::get().ativa(id) && state::obter(id).situacao != Situacao::Desconectando
}

/// Updates the tray icon, tooltip and menu from the shared state.
/// Runs on the main thread: called by the App and by the Win32 timer (which
/// works with the window hidden).
pub(crate) fn apply_tray_state() {
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

/// Creates the tray icon on the calling thread (Windows: the main thread;
/// Linux: the GTK thread of the tray).
pub(crate) fn criar_bandeja() {
    let (g, gw, gh) = load_icon_rgba(include_bytes!("../assets/gray_32.png"));
    let (w_, ww, wh) = load_icon_rgba(include_bytes!("../assets/warn_32.png"));
    let (o, ow, oh) = load_icon_rgba(include_bytes!("../assets/ok_32.png"));
    let icon_gray = tray_icon::Icon::from_rgba(g, gw, gh).unwrap();
    let icon_warn = tray_icon::Icon::from_rgba(w_, ww, wh).unwrap();
    let icon_ok = tray_icon::Icon::from_rgba(o, ow, oh).unwrap();

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(Menu::new()))
        .with_menu_on_left_click(false)
        .with_tooltip(format!(
            "{APP_TITLE} - {}",
            tr!("Desconectado", "Disconnected")
        ))
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
}

/// Turns an account on or off. Used by the UI and by the tray menu.
/// Asks for confirmation when two connections would fight over the default route.
pub(crate) fn alternar_conta(id: &str) -> Result<(), ErroConexao> {
    let m = engine::get();
    if m.ativa(id) {
        m.desconectar(id);
        return Ok(());
    }
    let conflitos = m.conflitos_de_rota(id);
    if !conflitos.is_empty() {
        let nome = m
            .conta(id)
            .map(|c| c.nome_exibicao().to_string())
            .unwrap_or_default();
        let sim = dialogs::sim_nao(
            &trf!(
                "\"{nome}\" e \"{}\" mandam todo o tráfego da internet pela VPN.\n\n\
                 Com as duas conectadas, só a última funciona como rota padrão \
                 (a outra continua acessando apenas a própria rede). Conectar mesmo assim?",
                "\"{nome}\" and \"{}\" both send all internet traffic through the VPN.\n\n\
                 With both connected, only the last one works as the default route \
                 (the other one keeps reaching only its own network). Connect anyway?",
                conflitos.join("\", \"")
            ),
            dialogs::Nivel::Aviso,
        );
        if !sim {
            return Ok(());
        }
    }
    m.conectar(id, find_openvpn())
}

pub(crate) fn install_tray_handlers() {
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(acoes
            .iter()
            .any(|(_, a)| matches!(a, AcaoMenu::DesconectarTodas)));
        assert!(acoes.iter().any(|(_, a)| matches!(a, AcaoMenu::Sair)));

        assert!(!acoes.iter().any(|(_, a)| matches!(a, AcaoMenu::Atualizar)));

        // a single account: no "Disconnect all"
        let (_m, _i, acoes) = montar_menu(&contas[..1], None);
        assert!(!acoes
            .iter()
            .any(|(_, a)| matches!(a, AcaoMenu::DesconectarTodas)));

        // new version published: item to update
        let (_m, _i, acoes) = montar_menu(&contas, Some("1.0.1"));
        assert!(acoes.iter().any(|(_, a)| matches!(a, AcaoMenu::Atualizar)));
    }
}
