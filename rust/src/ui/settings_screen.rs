//! Settings screen.

use super::*;

impl App {
    /// Settings: startup, appearance and updates, in cards like the accounts.
    pub(super) fn tela_configuracoes(&mut self, ui: &mut egui::Ui) {
        use i18n::Idioma;
        use update::Estado;
        let m = engine::get();
        let dark = self.dark;
        let fraco = label_color(dark);
        let pequeno = |t: &str| egui::RichText::new(t).small().color(fraco);

        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(tr!("Configurações", "Settings"))
                    .size(18.0)
                    .strong(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                link_codigo_fonte(ui, &self.logo_github, dark);
            });
        });
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // ---------------------------------------------------- startup
                secao(ui, tr!("INICIALIZAÇÃO", "STARTUP"), dark, |ui| {
                    let mut com_windows = startup::ativo();
                    if linha_toggle(
                        ui,
                        dark,
                        if cfg!(windows) {
                            tr!("Iniciar com o Windows", "Start with Windows")
                        } else {
                            tr!("Iniciar com o sistema", "Start with the system")
                        },
                        if cfg!(windows) {
                            tr!(
                                "Abre o app quando você entra no Windows.",
                                "Opens the app when you sign in to Windows."
                            )
                        } else {
                            tr!("Abre o app quando você entra na sessão.", "Opens the app when you sign in.")
                        },
                        &mut com_windows,
                        true,
                    ) {
                        if let Err(e) = startup::definir(com_windows) {
                            error_box(&e);
                        }
                    }
                    ui.add_space(4.0);
                    let mut minimizado = m.iniciar_minimizado();
                    if linha_toggle(
                        ui,
                        dark,
                        tr!("Iniciar minimizado", "Start minimized"),
                        if cfg!(windows) {
                            tr!(
                                "Ao abrir com o Windows, fica só no ícone da bandeja.",
                                "When opened by Windows, stays in the tray icon only."
                            )
                        } else {
                            tr!(
                                "Ao abrir com o sistema, fica só no ícone da bandeja.",
                                "When opened by the system, stays in the tray icon only."
                            )
                        },
                        &mut minimizado,
                        com_windows,
                    ) {
                        m.salvar_iniciar_minimizado(minimizado);
                    }
                });

                // ------------------------------------------------ notifications
                secao(ui, tr!("AVISOS", "NOTIFICATIONS"), dark, |ui| {
                    let mut avisos = m.avisos_de_queda();
                    if linha_toggle(
                        ui,
                        dark,
                        tr!("Avisar quando uma VPN cair", "Notify when a VPN drops"),
                        if cfg!(windows) {
                            tr!(
                                "Notificação do Windows quando a conexão cai e quando volta.",
                                "Windows notification when the connection drops and when it is back."
                            )
                        } else {
                            tr!(
                                "Notificação na área de trabalho quando a conexão cai e quando volta.",
                                "Desktop notification when the connection drops and when it is back."
                            )
                        },
                        &mut avisos,
                        true,
                    ) {
                        m.salvar_avisos_de_queda(avisos);
                    }
                });

                // ------------------------------------------------- appearance
                secao(ui, tr!("APARÊNCIA", "APPEARANCE"), dark, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(tr!("Tema", "Theme")).color(ui.visuals().strong_text_color()));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            // right to left: the last one added is the leftmost
                            for (escuro, nome) in [(false, tr!("Claro", "Light")), (true, tr!("Escuro", "Dark"))] {
                                if ui.selectable_label(self.dark == escuro, nome).clicked() && self.dark != escuro {
                                    self.dark = escuro;
                                    apply_style(ui.ctx(), escuro);
                                    m.salvar_tema(escuro);
                                }
                            }
                        });
                    });
                    ui.add_space(4.0);
                    let escolhido = m.idioma();
                    let automatico = trf!("Automático ({})", "Automatic ({})", i18n::do_sistema().nome());
                    let mut novo = escolhido;
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(tr!("Idioma", "Language")).color(ui.visuals().strong_text_color()));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            egui::ComboBox::from_id_salt("idioma")
                                .selected_text(escolhido.map(Idioma::nome).unwrap_or(&automatico))
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut novo, None, automatico.as_str());
                                    for i in [Idioma::Portugues, Idioma::Ingles] {
                                        ui.selectable_value(&mut novo, Some(i), i.nome());
                                    }
                                });
                        });
                    });
                    if novo != escolhido {
                        m.salvar_idioma(novo);
                        apply_tray_state();
                    }
                });

                // ---------------------------------------------------- updates
                secao(ui, tr!("ATUALIZAÇÕES", "UPDATES"), dark, |ui| {
                    let mut auto = m.verifica_atualizacoes();
                    if linha_toggle(
                        ui,
                        dark,
                        tr!("Procurar novas versões", "Check for new versions"),
                        tr!(
                            "A cada 6 horas, no GitHub, sem enviar dados seus.",
                            "Every 6 hours, on GitHub, without sending any of your data."
                        ),
                        &mut auto,
                        true,
                    ) {
                        m.salvar_verifica_atualizacoes(auto);
                    }
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(pequeno(&trf!("Versão {}", "Version {}", update::VERSAO_ATUAL)));
                        let estado = update::estado();
                        match &estado {
                            Estado::Verificando => {
                                ui.spinner();
                                ui.label(pequeno(tr!("procurando...", "checking...")));
                            }
                            Estado::EmDia => {
                                ui.label(pequeno(tr!(
                                    "·  você já tem a versão mais recente",
                                    "·  you have the latest version"
                                )));
                            }
                            Estado::FalhaVerificacao(e) => {
                                ui.add(egui::Label::new(pequeno(&format!("·  {e}"))).truncate());
                            }
                            Estado::Nada => {}
                            _ => {
                                if let Some(v) = update::disponivel() {
                                    let texto = egui::RichText::new(trf!(
                                        "·  versão {v} disponível",
                                        "·  version {v} available"
                                    ))
                                    .small()
                                    .color(ACCENT);
                                    if ui.link(texto).clicked() {
                                        self.janela_atualizacao = true;
                                    }
                                }
                            }
                        }
                        if matches!(estado, Estado::Nada | Estado::EmDia | Estado::FalhaVerificacao(_))
                            && ui
                                .link(egui::RichText::new(tr!("Procurar agora", "Check now")).small())
                                .clicked()
                        {
                            update::verificar_agora();
                        }
                    });
                });

                // ----------------------------------------------------- backup
                secao(ui, tr!("BACKUP DAS CONTAS", "ACCOUNT BACKUP"), dark, |ui| {
                    ui.label(pequeno(if cfg!(windows) {
                        tr!(
                            "Para levar as contas a outro computador ou conta do Windows: um \
                             arquivo protegido por senha, com usuários, senhas, seeds e os \
                             arquivos .ovpn.",
                            "To take the accounts to another computer or Windows account: a \
                             password-protected file with usernames, passwords, seeds and the \
                             .ovpn files."
                        )
                    } else {
                        tr!(
                            "Para levar as contas a outro computador ou usuário: um arquivo \
                             protegido por senha, com usuários, senhas, seeds e os arquivos .ovpn.",
                            "To take the accounts to another computer or user: a \
                             password-protected file with usernames, passwords, seeds and the \
                             .ovpn files."
                        )
                    }));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let tem_contas = !m.contas().is_empty();
                        if ui.add_enabled(tem_contas, egui::Button::new(tr!("Exportar...", "Export..."))).clicked() {
                            self.backup = Some(JanelaBackup::nova(true, PathBuf::new()));
                        }
                        if ui.button(tr!("Importar...", "Import...")).clicked() {
                            if let Some(p) = dialogs::abrir_arquivo(
                                tr!("Escolha o backup das contas", "Choose the account backup"),
                                filtro_backup(),
                                None,
                            ) {
                                self.backup = Some(JanelaBackup::nova(false, p));
                            }
                        }
                    });
                });
                // VPN_SCREENSHOT=settings-end: the end of the screen (screenshots)
                if std::env::var("VPN_SCREENSHOT").as_deref() == Ok("settings-end") {
                    ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
                }
            });
    }
}
