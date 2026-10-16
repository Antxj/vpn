//! Colors and style of the window (dark and light themes).

use super::*;
use crate::state::Situacao;

pub(super) const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x3b, 0x82, 0xf6);

/// Color of the field labels (more readable than egui's default "weak").
pub(super) fn label_color(dark: bool) -> egui::Color32 {
    if dark {
        egui::Color32::from_gray(185)
    } else {
        egui::Color32::from_gray(95)
    }
}

pub(super) fn apply_style(ctx: &egui::Context, dark: bool) {
    let mut visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    let rounding = egui::Rounding::same(6.0);
    visuals.widgets.noninteractive.rounding = rounding;
    visuals.widgets.inactive.rounding = rounding;
    visuals.widgets.hovered.rounding = rounding;
    visuals.widgets.active.rounding = rounding;
    visuals.widgets.open.rounding = rounding;
    visuals.selection.bg_fill = ACCENT;
    visuals.hyperlink_color = ACCENT;

    // the app's own theme, whatever the Windows one: egui keeps a dark and a
    // light style and, by default, picks one by the Windows theme (with
    // Windows in light mode it showed egui's plain style with dark cards)
    let tema = if dark {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    };
    ctx.set_theme(tema);
    let mut style = (*ctx.style()).clone();
    style.visuals = visuals;
    use egui::{FontFamily, FontId, TextStyle};
    style.text_styles = [
        (
            TextStyle::Heading,
            FontId::new(24.0, FontFamily::Proportional),
        ),
        (TextStyle::Body, FontId::new(15.0, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(15.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Small,
            FontId::new(12.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(12.5, FontFamily::Monospace),
        ),
    ]
    .into();
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(14.0, 8.0);
    // egui's default (500 px) is wider than the 480 px window: long tooltips
    // were cut at the window edge
    style.spacing.tooltip_width = 320.0;
    ctx.set_style_of(tema, style);
}

pub(super) fn cor_situacao(s: Situacao, padrao: egui::Color32) -> egui::Color32 {
    match s {
        Situacao::Conectado => egui::Color32::from_rgb(0x2a, 0xa0, 0x2a),
        Situacao::Conectando | Situacao::Reconectando | Situacao::Desconectando => {
            egui::Color32::from_rgb(0xb5, 0x89, 0x00)
        }
        Situacao::Desconectado => padrao,
    }
}

/// Soft color of each account in the log (by position in the list).
pub(super) fn cor_da_conta(i: usize, dark: bool) -> egui::Color32 {
    const ESCURO: [(u8, u8, u8); 5] = [
        (0x60, 0xa5, 0xfa),
        (0x34, 0xd3, 0x99),
        (0xfb, 0xbf, 0x24),
        (0xf4, 0x72, 0xb6),
        (0xa7, 0x8b, 0xfa),
    ];
    const CLARO: [(u8, u8, u8); 5] = [
        (0x1d, 0x4e, 0xd8),
        (0x04, 0x78, 0x57),
        (0xb4, 0x53, 0x09),
        (0xbe, 0x18, 0x5d),
        (0x6d, 0x28, 0xd9),
    ];
    let (r, g, b) = if dark { ESCURO[i % 5] } else { CLARO[i % 5] };
    egui::Color32::from_rgb(r, g, b)
}

/// Background of the cards (accounts, settings sections).
pub(super) fn fundo_cartao(dark: bool) -> egui::Color32 {
    if dark {
        egui::Color32::from_rgb(0x26, 0x29, 0x31)
    } else {
        egui::Color32::from_rgb(0xff, 0xff, 0xff)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every symbol shown in the UI MUST exist in egui's fonts, otherwise it
    /// becomes a "box". This test locks the approved list.
    #[test]
    fn glifos_presentes_nas_fontes_do_egui() {
        let defs = egui::FontDefinitions::default();
        // what matters is the Proportional family (used in the UI texts)
        let proportional = &defs.families[&egui::FontFamily::Proportional];
        let covered = |c: char| {
            proportional.iter().any(|name| {
                defs.font_data.get(name).is_some_and(|data| {
                    ttf_parser::Face::parse(&data.font, 0)
                        .map(|f| f.glyph_index(c).is_some())
                        .unwrap_or(false)
                })
            })
        };
        // symbols actually used in the UI:
        for c in ['\u{2022}', '·', '\u{2B07}', '\u{2B06}', '—', '⚙'] {
            assert!(covered(c), "egui proportional font lacks {c:?}");
        }
    }

    /// Regression (v1.0.2): tooltips wider than the window were cut at its edge.
    /// The app's theme wins over the Windows one: with Windows in light mode
    /// the dark theme stayed half applied (dark cards on egui's plain style).
    #[test]
    fn tema_do_app_vale_com_qualquer_tema_do_windows() {
        for sistema in [egui::Theme::Light, egui::Theme::Dark] {
            for escuro in [true, false] {
                let ctx = egui::Context::default();
                apply_style(&ctx, escuro);
                let entrada = egui::RawInput {
                    system_theme: Some(sistema),
                    ..Default::default()
                };
                let _ = ctx.run(entrada, |_| {});
                let estilo = ctx.style();
                assert_eq!(
                    estilo.visuals.dark_mode, escuro,
                    "{sistema:?}, dark {escuro}"
                );
                // our fonts, not egui's defaults
                assert_eq!(estilo.text_styles[&egui::TextStyle::Body].size, 15.0);
            }
        }
    }

    #[test]
    fn tooltips_fit_inside_the_window() {
        let ctx = egui::Context::default();
        for dark in [true, false] {
            apply_style(&ctx, dark);
            let largura = ctx.style().spacing.tooltip_width;
            // room for the tooltip frame and the window border
            assert!(
                largura + 40.0 <= WINDOW_SIZE[0],
                "tooltip width {largura} does not fit"
            );
        }
    }
}
