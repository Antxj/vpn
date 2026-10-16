//! Accounts screen: list, order and removal of the accounts.

use super::*;

impl App {
    pub(super) fn abrir_editor(&mut self, conta: Conta, nova: bool) {
        self.tela = Tela::Contas;
        self.editor = Some(Editor {
            conta,
            nova,
            mostrar_seed: false,
            mostrar_senha: false,
            erro: None,
        });
    }

    pub(super) fn tela_contas(&mut self, ui: &mut egui::Ui) {
        if self.editor.is_some() {
            self.tela_editor(ui);
            return;
        }
        let m = engine::get();
        let contas = m.contas();
        let fundo_cartao = fundo_cartao(self.dark);

        ui.label(
            egui::RichText::new(tr!("Contas", "Accounts"))
                .size(18.0)
                .strong(),
        );
        ui.add_space(4.0);
        let mut editar: Option<Conta> = None;
        let mut remover: Option<Conta> = None;

        egui::ScrollArea::vertical()
            .max_height((ui.available_height() - 60.0).max(120.0))
            .auto_shrink([false, true])
            .show(ui, |ui| {
                if contas.is_empty() {
                    ui.label(
                        egui::RichText::new(tr!(
                            "Nenhuma conta ainda. Adicione a primeira abaixo.",
                            "No accounts yet. Add the first one below."
                        ))
                        .color(label_color(self.dark)),
                    );
                }
                for conta in &contas {
                    let ativa = m.ativa(&conta.id);
                    egui::Frame::none()
                        .fill(fundo_cartao)
                        .rounding(8.0)
                        .inner_margin(egui::Margin::symmetric(12.0, 10.0))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                // name on the buttons row, without invading their space
                                let largura_nome = (ui.available_width() - 200.0).max(80.0);
                                ui.vertical(|ui| {
                                    ui.set_max_width(largura_nome);
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(conta.nome_exibicao()).strong(),
                                        )
                                        .truncate(),
                                    );
                                });
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let dica = tr!(
                                            "Desconecte a conta antes de alterá-la.",
                                            "Disconnect the account before changing it."
                                        );
                                        let r = ui.add_enabled(
                                            !ativa,
                                            egui::Button::new(tr!("Remover", "Remove")),
                                        );
                                        if r.clicked() {
                                            remover = Some(conta.clone());
                                        }
                                        if ativa {
                                            r.on_disabled_hover_text(dica);
                                        }
                                        let r = ui.add_enabled(
                                            !ativa,
                                            egui::Button::new(tr!("Editar", "Edit")),
                                        );
                                        if r.clicked() {
                                            editar = Some(conta.clone());
                                        }
                                        if ativa {
                                            r.on_disabled_hover_text(dica);
                                        }
                                    },
                                );
                            });
                            // file and authentication get the full width
                            let arquivo = conta.arquivo();
                            let mut detalhes = format!(
                                "{}  ·  {}",
                                if arquivo.is_empty() {
                                    tr!("sem arquivo .ovpn", "no .ovpn file")
                                } else {
                                    &arquivo
                                },
                                conta.autenticacao.rotulo()
                            );
                            if let Some((tipo, _)) = tipo_de_tunel(conta) {
                                detalhes.push_str("  ·  ");
                                detalhes.push_str(tipo);
                            }
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(detalhes)
                                        .small()
                                        .color(label_color(self.dark)),
                                )
                                .truncate(),
                            );
                        });
                    ui.add_space(6.0);
                }
            });

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let nova = egui::Button::new(
                egui::RichText::new(tr!("Nova conta", "New account")).color(egui::Color32::WHITE),
            )
            .fill(ACCENT);
            // saved accounts unreadable (Linux keyring): nothing could be saved
            if ui
                .add_enabled(!settings::contas_ilegiveis(), nova)
                .clicked()
            {
                let mut c = Conta::nova();
                c.config = find_default_config()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
                ajustar_autenticacao(&mut c);
                self.abrir_editor(c, true);
            }
        });

        if let Some(c) = editar {
            self.abrir_editor(c, false);
        }
        if let Some(c) = remover {
            let sim = dialogs::sim_nao(
                &trf!(
                    "Remover a conta \"{}\"?\n\nUsuário, seed e senha salvos dela serão apagados.",
                    "Remove the account \"{}\"?\n\nIts saved username, seed and password will be deleted.",
                    c.nome_exibicao()
                ),
                dialogs::Nivel::Aviso,
            );
            if sim {
                m.remover_conta(&c.id);
            }
        }
    }
}
