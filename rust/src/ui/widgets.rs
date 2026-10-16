//! Controls shared by the screens (switch, tabs, sections, source code link).

use super::*;

/// Drawn toggle switch: on = blue.
pub(super) fn toggle(ui: &mut egui::Ui, ligado: bool, habilitado: bool) -> egui::Response {
    let tamanho = egui::vec2(44.0, 24.0);
    let sentido = if habilitado {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(tamanho, sentido);
    if ui.is_rect_visible(rect) {
        let anim = ui.ctx().animate_bool(response.id, ligado);
        let fundo = if ligado {
            ACCENT
        } else if ui.visuals().dark_mode {
            egui::Color32::from_gray(70)
        } else {
            egui::Color32::from_gray(190)
        };
        let fundo = if habilitado {
            fundo
        } else {
            fundo.gamma_multiply(0.35)
        };
        let raio = rect.height() / 2.0;
        ui.painter().rect_filled(rect, raio, fundo);
        let x = egui::lerp((rect.left() + raio)..=(rect.right() - raio), anim);
        ui.painter().circle_filled(
            egui::pos2(x, rect.center().y),
            raio - 3.0,
            egui::Color32::WHITE,
        );
    }
    if habilitado {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    }
}

/// Discreet log tab: small text, underlined when selected. Returns true when clicked.
pub(super) fn aba(
    ui: &mut egui::Ui,
    texto: &str,
    ativa: bool,
    cor: Option<egui::Color32>,
    dark: bool,
) -> bool {
    let cor_texto = if ativa {
        cor.unwrap_or_else(|| ui.visuals().strong_text_color())
    } else {
        label_color(dark)
    };
    let mut rich = egui::RichText::new(texto).small().color(cor_texto);
    if ativa {
        rich = rich.strong();
    }
    let r = ui
        .add(egui::Label::new(rich).sense(egui::Sense::click()))
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if ativa {
        let y = r.rect.bottom() + 2.0;
        ui.painter().line_segment(
            [egui::pos2(r.rect.left(), y), egui::pos2(r.rect.right(), y)],
            egui::Stroke::new(2.0_f32, cor.unwrap_or(ACCENT)),
        );
    }
    r.clicked()
}

/// Discreet "source code" link beside the settings title: GitHub mark and
/// small gray text, which light up on hover. Opens the repository.
pub(super) fn link_codigo_fonte(ui: &mut egui::Ui, logo: &egui::TextureHandle, dark: bool) {
    let texto = tr!("Código-fonte", "Source code");
    let url = update::pagina_repositorio();
    let galley = egui::WidgetText::from(egui::RichText::new(texto).small()).into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Small,
    );
    let icone = 14.0;
    let espaco = 5.0;
    let tamanho = egui::vec2(icone + espaco + galley.size().x, icone.max(galley.size().y));
    let (rect, resposta) = ui.allocate_exact_size(tamanho, egui::Sense::click());
    resposta.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Link, true, texto));
    if ui.is_rect_visible(rect) {
        let cor = if resposta.hovered() {
            ui.visuals().strong_text_color()
        } else {
            label_color(dark)
        };
        let r_icone = egui::Rect::from_min_size(
            egui::pos2(rect.left(), rect.center().y - icone / 2.0),
            egui::vec2(icone, icone),
        );
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        ui.painter().image(logo.id(), r_icone, uv, cor);
        let pos = egui::pos2(
            r_icone.right() + espaco,
            rect.center().y - galley.size().y / 2.0,
        );
        ui.painter()
            .galley_with_override_text_color(pos, galley, cor);
    }
    let resposta = resposta
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(format!(
            "{url}\n{}",
            tr!("Licença GPL-3.0", "GPL-3.0 license")
        ));
    if resposta.clicked() {
        let _ = open::that(&url);
    }
}

/// Settings section: small title above a card.
pub(super) fn secao(
    ui: &mut egui::Ui,
    titulo: &str,
    dark: bool,
    conteudo: impl FnOnce(&mut egui::Ui),
) {
    ui.add_space(8.0);
    rotulo(ui, titulo, dark);
    egui::Frame::none()
        .fill(fundo_cartao(dark))
        .rounding(8.0)
        .inner_margin(egui::Margin::symmetric(12.0, 10.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            conteudo(ui);
        });
}

/// Settings row: title and description on the left, switch on the right.
/// Returns true when the user flipped the switch.
pub(super) fn linha_toggle(
    ui: &mut egui::Ui,
    dark: bool,
    titulo: &str,
    descricao: &str,
    valor: &mut bool,
    habilitado: bool,
) -> bool {
    let mut mudou = false;
    ui.horizontal(|ui| {
        let largura = (ui.available_width() - 64.0).max(80.0);
        ui.vertical(|ui| {
            ui.set_max_width(largura);
            let cor = if habilitado {
                ui.visuals().strong_text_color()
            } else if dark {
                egui::Color32::from_gray(110)
            } else {
                egui::Color32::from_gray(165)
            };
            ui.add(egui::Label::new(egui::RichText::new(titulo).color(cor)).truncate());
            if !descricao.is_empty() {
                ui.label(
                    egui::RichText::new(descricao)
                        .small()
                        .color(label_color(dark)),
                );
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if toggle(ui, *valor, habilitado).clicked() {
                *valor = !*valor;
                mudou = true;
            }
        });
    });
    mudou
}

pub(super) fn rotulo(ui: &mut egui::Ui, texto: &str, dark: bool) {
    ui.label(egui::RichText::new(texto).small().color(label_color(dark)));
}
