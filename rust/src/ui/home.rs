//! Home screen: the accounts with their switches, traffic and the log.

use super::*;
use crate::state::Situacao;

/// Account tunnel type, for the card: (short text, explanation).
pub(super) fn tipo_de_tunel(conta: &Conta) -> Option<(&'static str, &'static str)> {
    conta.tunel_completo().map(|completo| {
        if completo {
            (
                tr!("toda a internet", "all traffic"),
                tr!(
                    "Toda a internet passa por esta VPN (túnel completo).",
                    "All internet traffic goes through this VPN (full tunnel)."
                ),
            )
        } else {
            (
                tr!("só a rede da VPN", "VPN network only"),
                tr!(
                    "Só a rede da VPN passa por ela; o resto usa a sua internet \
                     normal (túnel dividido).",
                    "Only the VPN's network goes through it; everything else uses \
                     your regular internet (split tunnel)."
                ),
            )
        }
    })
}

/// Accounts that appear in the log, in the order of the account list.
pub(super) fn abas_do_log(contas: &[Conta]) -> Vec<String> {
    let entradas = state::log_entradas();
    contas
        .iter()
        .map(|c| c.nome_exibicao().to_string())
        .filter(|n| entradas.iter().any(|e| &e.origem == n))
        .collect()
}

impl App {
    pub(super) fn tela_inicio(&mut self, ui: &mut egui::Ui) {
        let m = engine::get();
        let contas = m.contas();

        if contas.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new(tr!("Nenhuma conta cadastrada.", "No accounts yet."))
                        .size(16.0),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(tr!(
                        "Cadastre a primeira VPN para conectar.",
                        "Add your first VPN to connect."
                    ))
                    .color(label_color(self.dark)),
                );
                ui.add_space(12.0);
                let btn = egui::Button::new(
                    egui::RichText::new(tr!("Adicionar conta", "Add account"))
                        .color(egui::Color32::WHITE),
                )
                .fill(ACCENT);
                if ui.add(btn).clicked() {
                    self.abrir_editor(Conta::nova(), true);
                }
            });
            return;
        }

        let fundo_cartao = fundo_cartao(self.dark);
        // the list takes the top space and the log is anchored at the bottom
        let varias_ativas = contas.iter().filter(|c| m.ativa(&c.id)).count() > 1;
        let altura_log = 150.0
            + if varias_ativas { 44.0 } else { 0.0 }
            + if abas_do_log(&contas).len() >= 2 {
                26.0
            } else {
                0.0
            };
        let altura_lista = (ui.available_height() - altura_log).max(120.0);
        let mut erro: Option<ErroConexao> = None;

        egui::ScrollArea::vertical()
            .max_height(altura_lista)
            .min_scrolled_height(altura_lista)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for conta in &contas {
                    let e = state::obter(&conta.id);
                    let ligada = conta_ligada(&conta.id);
                    egui::Frame::none()
                        .fill(fundo_cartao)
                        .rounding(8.0)
                        .inner_margin(egui::Margin::symmetric(12.0, 10.0))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                // the text never invades the toggle's space:
                                // whatever does not fit ends with an ellipsis
                                let largura_texto = (ui.available_width() - 64.0).max(80.0);
                                let fraco = label_color(self.dark);
                                ui.vertical(|ui| {
                                    ui.set_max_width(largura_texto);
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(conta.nome_exibicao())
                                                .strong()
                                                .size(16.0),
                                        )
                                        .truncate(),
                                    );
                                    let cor = cor_situacao(e.situacao, fraco);
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 6.0;
                                        ui.label(
                                            egui::RichText::new(format!("\u{2022}  {}", e.texto()))
                                                .color(cor),
                                        );
                                        if let Some((tipo, dica)) = tipo_de_tunel(conta) {
                                            ui.label(
                                                egui::RichText::new(format!("·  {tipo}"))
                                                    .small()
                                                    .color(fraco),
                                            )
                                            .on_hover_text(dica);
                                        }
                                    });
                                    if let Some(t) = e.trafego {
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(format!(
                                                    "\u{2B07} {}    \u{2B06} {}",
                                                    fmt_rate(t.down_rate),
                                                    fmt_rate(t.up_rate)
                                                ))
                                                .small()
                                                .color(fraco),
                                            )
                                            .truncate(),
                                        );
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(trf!(
                                                    "recebido {}  ·  enviado {}",
                                                    "received {}  ·  sent {}",
                                                    fmt_bytes(t.down_total as f64),
                                                    fmt_bytes(t.up_total as f64)
                                                ))
                                                .small()
                                                .color(fraco),
                                            )
                                            .truncate(),
                                        );
                                    }
                                });
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let ocupada = e.situacao == Situacao::Desconectando;
                                        let r =
                                            toggle(ui, ligada, !ocupada).on_hover_text(if ligada {
                                                tr!("Desconectar", "Disconnect")
                                            } else {
                                                tr!("Conectar", "Connect")
                                            });
                                        if r.clicked() {
                                            if let Err(err) = alternar_conta(&conta.id) {
                                                erro = Some(err);
                                            }
                                        }
                                    },
                                );
                            });
                        });
                    ui.add_space(6.0);
                }
            });

        if let Some(e) = erro {
            self.tratar_erro(e);
        }

        if varias_ativas {
            ui.vertical_centered(|ui| {
                if ui
                    .button(tr!("Desconectar todas", "Disconnect all"))
                    .clicked()
                {
                    m.desconectar_todas();
                }
            });
        }
        ui.add_space(4.0);
        self.painel_log(ui);
    }

    /// Connection log. With two or more accounts in it, discreet tabs on top:
    /// "All" (every account, each name in its own color) or one account only.
    pub(super) fn painel_log(&mut self, ui: &mut egui::Ui) {
        let dark = self.dark;
        let contas = engine::get().contas();
        let abas = abas_do_log(&contas);
        if self.aba_log.as_ref().is_some_and(|a| !abas.contains(a)) {
            self.aba_log = None;
        }
        let entradas: Vec<state::Entrada> = state::log_entradas()
            .into_iter()
            .filter(|e| self.aba_log.as_ref().is_none_or(|a| &e.origem == a))
            .collect();

        egui::Frame::none()
            .fill(if dark {
                egui::Color32::from_rgb(0x14, 0x16, 0x1a)
            } else {
                egui::Color32::from_rgb(0xff, 0xff, 0xff)
            })
            .rounding(6.0)
            .inner_margin(egui::Margin::same(8.0))
            .show(ui, |ui| {
                ui.set_min_height(110.0);
                if abas.len() >= 2 || !entradas.is_empty() {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 12.0;
                        if abas.len() >= 2 {
                            if aba(ui, tr!("Todas", "All"), self.aba_log.is_none(), None, dark) {
                                self.aba_log = None;
                            }
                            for (i, nome) in abas.iter().enumerate() {
                                let ativa = self.aba_log.as_deref() == Some(nome.as_str());
                                if aba(ui, nome, ativa, Some(cor_da_conta(i, dark)), dark) {
                                    self.aba_log = Some(nome.clone());
                                }
                            }
                        }
                        if !entradas.is_empty() {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    self.botao_copiar_log(ui, &entradas, &contas);
                                },
                            );
                        }
                    });
                    ui.add_space(4.0);
                }
                egui::ScrollArea::vertical()
                    // each tab keeps its own scroll position
                    .id_salt(("log", self.aba_log.clone()))
                    .max_height(120.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        if entradas.is_empty() {
                            ui.label(
                                egui::RichText::new(tr!(
                                    "As mensagens das conexões aparecem aqui.",
                                    "Connection messages appear here."
                                ))
                                .small()
                                .color(label_color(dark)),
                            );
                        }
                        // same size and font as the log always had (small text)
                        let fonte = egui::FontId::proportional(12.0);
                        let normal = ui.visuals().text_color();
                        for e in &entradas {
                            let mut linha = egui::text::LayoutJob::default();
                            let mut parte = |texto: &str, cor: egui::Color32| {
                                linha.append(
                                    texto,
                                    0.0,
                                    egui::TextFormat::simple(fonte.clone(), cor),
                                );
                            };
                            parte(&format!("{} ", e.hora), label_color(dark));
                            // in "All", the account name in its color; in one
                            // account's tab the name would be redundant
                            if self.aba_log.is_none() && !e.origem.is_empty() {
                                let cor = abas
                                    .iter()
                                    .position(|n| n == &e.origem)
                                    .map(|i| cor_da_conta(i, dark))
                                    .unwrap_or(normal);
                                parte(&format!("[{}] ", e.origem), cor);
                            }
                            parte(&e.msg, normal);
                            linha.wrap.max_width = ui.available_width();
                            ui.label(linha);
                        }
                    });
            });
    }

    /// "Copy" on the log: the lines of the open tab, without secrets or
    /// usernames, ready to paste in a message to whoever gives support.
    pub(super) fn botao_copiar_log(
        &mut self,
        ui: &mut egui::Ui,
        entradas: &[state::Entrada],
        contas: &[Conta],
    ) {
        const FEEDBACK: Duration = Duration::from_secs(2);
        let copiado = self.log_copiado.is_some_and(|t| t.elapsed() < FEEDBACK);
        let rotulo = if copiado {
            tr!("Copiado!", "Copied!")
        } else {
            tr!("Copiar", "Copy")
        };
        let r = ui
            .add(
                egui::Label::new(
                    egui::RichText::new(rotulo)
                        .small()
                        .color(label_color(self.dark)),
                )
                .sense(egui::Sense::click()),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(tr!(
                "Copia este log para mandar a quem dá suporte. Senhas, seeds, \
                 usuários e o nome do computador são escondidos.",
                "Copies this log to send to whoever gives you support. Passwords, \
                 seeds, usernames and the computer name are hidden."
            ));
        if r.clicked() && !copiado {
            let conta = self
                .aba_log
                .as_ref()
                .and_then(|nome| contas.iter().find(|c| c.nome_exibicao() == nome.as_str()));
            let texto = support::texto_do_log(entradas, conta, contas);
            match arboard::Clipboard::new().and_then(|mut c| c.set_text(texto)) {
                Ok(()) => {
                    self.log_copiado = Some(Instant::now());
                    ui.ctx().request_repaint_after(FEEDBACK);
                }
                Err(e) => error_box(&trf!("Não foi possível copiar: {e}", "Could not copy: {e}")),
            }
        }
    }
}
