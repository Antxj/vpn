//! Window that offers a new version and follows its download.

use super::*;

impl App {
    pub(super) fn janela_atualizacao(&mut self, ctx: &egui::Context) {
        use update::Estado;
        let estado = update::estado();
        let versao = match &estado {
            Estado::Disponivel(v)
            | Estado::Baixando(v, _)
            | Estado::FalhaInstalacao(v, _)
            | Estado::Reiniciando(v) => v.clone(),
            // VPN_SCREENSHOT=update opens before the check finishes
            _ => return,
        };
        let dark = self.dark;
        let ocupada = matches!(estado, Estado::Baixando(..) | Estado::Reiniciando(_));
        let mut aberta = true;
        let mut fechar = false;
        let mut instalar = false;

        let mut janela = egui::Window::new(tr!("Atualização", "Update"))
            .id(egui::Id::new("janela_atualizacao"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]);
        if !ocupada {
            janela = janela.open(&mut aberta);
        }
        janela.show(ctx, |ui| {
            ui.set_max_width(380.0);
            match &estado {
                Estado::Disponivel(v) => {
                    ui.label(
                        egui::RichText::new(trf!(
                            "A versão {} está disponível.",
                            "Version {} is available.",
                            v.numero
                        ))
                        .strong(),
                    );
                    ui.label(
                        egui::RichText::new(trf!(
                            "Você usa a versão {}.",
                            "You are using version {}.",
                            update::VERSAO_ATUAL
                        ))
                        .color(label_color(dark)),
                    );
                    ui.add_space(6.0);
                    ui.label(if v.instalavel() {
                        tr!(
                            "O aplicativo baixa a versão nova, confere a integridade e reabre \
                             sozinho. As VPNs conectadas caem por alguns segundos e voltam em \
                             seguida.",
                            "The app downloads the new version, verifies it and reopens by \
                             itself. Connected VPNs drop for a few seconds and then come back."
                        )
                    } else {
                        tr!(
                            "Esta versão precisa ser baixada pela página.",
                            "This version must be downloaded from the release page."
                        )
                    });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if v.instalavel() {
                            let btn = egui::Button::new(
                                egui::RichText::new(tr!("Atualizar agora", "Update now"))
                                    .color(egui::Color32::WHITE),
                            )
                            .fill(ACCENT);
                            if ui.add(btn).clicked() {
                                instalar = true;
                            }
                        }
                        if ui.button(tr!("Ver novidades", "What's new")).clicked() {
                            let _ = open::that(&v.pagina);
                        }
                        if ui.button(tr!("Agora não", "Not now")).clicked() {
                            fechar = true;
                        }
                    });
                }
                Estado::Baixando(v, fracao) => {
                    ui.label(trf!(
                        "Baixando a versão {}...",
                        "Downloading version {}...",
                        v.numero
                    ));
                    ui.add(egui::ProgressBar::new(*fracao).show_percentage());
                }
                Estado::Reiniciando(v) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(trf!(
                            "Versão {} instalada. Reabrindo o aplicativo...",
                            "Version {} installed. Reopening the app...",
                            v.numero
                        ));
                    });
                }
                Estado::FalhaInstalacao(v, erro) => {
                    ui.colored_label(egui::Color32::from_rgb(0xdc, 0x26, 0x26), erro);
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if v.instalavel() && ui.button(tr!("Tentar de novo", "Try again")).clicked()
                        {
                            instalar = true;
                        }
                        if ui
                            .button(tr!("Abrir página da versão", "Open release page"))
                            .clicked()
                        {
                            let _ = open::that(&v.pagina);
                        }
                        if ui.button(tr!("Fechar", "Close")).clicked() {
                            fechar = true;
                        }
                    });
                }
                _ => {}
            }
        });
        if instalar {
            update::instalar(versao);
        }
        if fechar || !aberta {
            self.janela_atualizacao = false;
        }
    }
}
