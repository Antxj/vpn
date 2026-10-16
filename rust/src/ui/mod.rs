//! The window: app state, events of each frame and the screens
//! (one file per screen; colors in theme.rs, shared controls in widgets.rs).

mod accounts_screen;
mod backup_window;
mod editor;
mod home;
mod settings_screen;
mod theme;
mod update_window;
mod widgets;

use super::*;
use crate::accounts::Conta;
use crate::engine::ErroConexao;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use backup_window::*;
use editor::*;
use home::*;
use theme::*;
use widgets::*;

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

/// Export or import window of the account backup.
struct JanelaBackup {
    exportar: bool,
    /// Import: the chosen backup file.
    arquivo: PathBuf,
    senha: String,
    confirmar: String,
    mostrar: bool,
    erro: Option<String>,
    /// Import: accounts found after the password was accepted.
    contas: Option<Vec<backup::ContaExportada>>,
    /// Final message (the window then only offers "Close").
    concluido: Option<String>,
}

impl JanelaBackup {
    fn nova(exportar: bool, arquivo: PathBuf) -> Self {
        JanelaBackup {
            exportar,
            arquivo,
            senha: String::new(),
            confirmar: String::new(),
            mostrar: false,
            erro: None,
            contas: None,
            concluido: None,
        }
    }
}

pub(crate) struct App {
    dark: bool,
    tela: Tela,
    editor: Option<Editor>,
    qr_open: bool,
    /// Log tab: None = all accounts, Some(name) = only that account.
    aba_log: Option<String>,
    /// When "Copy" was clicked (shows "Copied!" for a moment).
    log_copiado: Option<Instant>,
    janela_atualizacao: bool,
    openvpn_missing: bool,
    last_ovpn_check: Instant,
    installing: bool,
    install_rx: Receiver<installer::Event>,
    install_tx: Sender<installer::Event>,
    backup: Option<JanelaBackup>,
    /// GitHub mark (white; tinted with the theme's text color).
    logo_github: egui::TextureHandle,
    /// Where the app runs from is not where it can stay (macOS).
    aviso_local: Option<String>,
}

impl App {
    pub(crate) fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let m = engine::get();
        let dark = m.tema_escuro();
        apply_style(&cc.egui_ctx, dark);
        let _ = EGUI_CTX.set(cc.egui_ctx.clone());
        let ctx = cc.egui_ctx.clone();
        state::ao_mudar(move || ctx.request_repaint());

        single::spawn_show_listener();

        desktop::iniciar_bandeja();
        desktop::ocultar_antes_do_primeiro_quadro();

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
                let nome = m
                    .conta(id)
                    .map(|c| c.nome_exibicao().to_string())
                    .unwrap_or_default();
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
                state::log(
                    nome,
                    trf!(
                        "Não conectou ao abrir: {}",
                        "Not connected on open: {}",
                        e.mensagem()
                    ),
                );
            }
        }
        let ctx = cc.egui_ctx.clone();
        update::ao_mudar(move || ctx.request_repaint());
        update::ao_achar_versao(desktop::notificar_nova_versao);
        update::iniciar_verificacao_periodica(|| engine::get().verifica_atualizacoes());

        let (rgba, w, h) = load_icon_rgba(include_bytes!("../../assets/github_28.png"));
        let logo_github = cc.egui_ctx.load_texture(
            "logo_github",
            egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba),
            egui::TextureOptions::LINEAR,
        );

        let mut app = Self {
            dark,
            // no accounts: open straight on the sign-up screen
            tela: if m.contas().is_empty() {
                Tela::Contas
            } else {
                Tela::Inicio
            },
            editor: None,
            qr_open: false,
            aba_log: None,
            log_copiado: None,
            janela_atualizacao: false,
            openvpn_missing: find_openvpn().is_none(),
            last_ovpn_check: Instant::now(),
            installing: false,
            install_rx,
            install_tx,
            logo_github,
            aviso_local: platform::paths::aviso_de_local(),
            backup: None,
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
            Ok("settings" | "settings-end") => app.tela = Tela::Configuracoes,
            Ok("export") => {
                app.tela = Tela::Configuracoes;
                app.backup = Some(JanelaBackup::nova(true, PathBuf::new()));
            }
            // "import:<file>": import window for that backup (screenshots)
            Ok(v) if v.starts_with("import:") => {
                app.tela = Tela::Configuracoes;
                app.backup = Some(JanelaBackup::nova(false, PathBuf::from(&v[7..])));
            }
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
        if dialogs::sim_nao(&pergunta, dialogs::Nivel::Aviso) {
            if instala {
                self.start_openvpn_install();
            } else {
                let _ = open::that(platform::openvpn::PAGINA_DE_DOWNLOAD);
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
                            state::log(
                                "",
                                trf!("Falha na instalação: {msg}", "Installation failed: {msg}"),
                            );
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

        // contas.dat exists but the secret store (Linux keyring) is locked
        if settings::contas_ilegiveis() {
            let mut de_novo = false;
            faixa(
                ui,
                tr!(
                    "Não consegui ler as contas salvas:\no chaveiro do sistema está trancado.",
                    "Could not read the saved accounts:\nthe system keyring is locked."
                ),
                &mut |ui| de_novo = ui.button(tr!("Tentar de novo", "Try again")).clicked(),
            );
            if de_novo && engine::get().ler_contas_de_novo() {
                let m = engine::get();
                i18n::aplicar(m.idioma());
                self.dark = m.tema_escuro();
                apply_style(ui.ctx(), self.dark);
                self.tela = if m.contas().is_empty() {
                    Tela::Contas
                } else {
                    Tela::Inicio
                };
            }
        }

        // macOS: the temporary copy of a downloaded app (updates would not last)
        if let Some(aviso) = &self.aviso_local {
            faixa(ui, aviso, &mut |_| {});
        }

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
                    let btn =
                        egui::Button::new(egui::RichText::new(rotulo).color(egui::Color32::WHITE))
                            .fill(ACCENT);
                    if ui.add(btn).clicked() {
                        if embutido {
                            instalar = true;
                        } else {
                            let _ = open::that(platform::openvpn::PAGINA_DE_DOWNLOAD);
                        }
                    }
                }
            });
            if instalar {
                self.start_openvpn_install();
            }
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.cumulative_pass_nr() == 2 {
            // the first frames were drawn
            renderer::opengl_funcionou();
        }
        desktop::por_quadro(ctx);
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
                            ui.add(
                                egui::Button::new(tr!("Voltar", "Back"))
                                    .min_size(egui::vec2(0.0, 32.0)),
                            )
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
                                if ui
                                    .add(b)
                                    .on_hover_text(tr!("Configurações", "Settings"))
                                    .clicked()
                                {
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
                                .on_hover_text(tr!(
                                    "Ver e instalar a atualização",
                                    "View and install the update"
                                ))
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
        if self.backup.is_some() {
            self.janela_backup(ctx);
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
