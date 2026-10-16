//! Account editor (new or existing account) and the QR code import.

use super::*;
use crate::accounts::Autenticacao;

/// The .ovpn decides whether there is a username/password at all: without
/// `auth-user-pass` the account becomes "certificate only".
pub(super) fn ajustar_autenticacao(conta: &mut Conta) {
    if conta.config.is_empty() {
        return;
    }
    let pede = accounts::pede_usuario_e_senha(std::path::Path::new(&conta.config));
    if !pede {
        conta.autenticacao = Autenticacao::SoCertificado;
    } else if conta.autenticacao == Autenticacao::SoCertificado {
        conta.autenticacao = Autenticacao::Token;
    }
}

impl App {
    pub(super) fn import_qr_image(&mut self, img: image::DynamicImage) {
        let Some(text) = qr::decode_qr(&img) else {
            error_box(tr!(
                "Não encontrei um QR Code nessa imagem.\n\
                 Confira se ele aparece inteiro e nítido.",
                "No QR code was found in this image.\n\
                 Make sure it is complete and sharp."
            ));
            return;
        };
        let data = qr::parse_payload(&text);
        let Some(seed) = data.seed else {
            let preview: String = text.chars().take(200).collect();
            error_box(&trf!(
                "Li o QR Code, mas não identifiquei uma seed nele.\n\nConteúdo:\n{preview}",
                "The QR code was read, but no seed was found in it.\n\nContent:\n{preview}"
            ));
            return;
        };
        if let Some(ed) = self.editor.as_mut() {
            ed.conta.seed = seed;
            if let Some(user) = data.user {
                ed.conta.usuario = user;
            }
            ed.erro = None;
        }
        self.qr_open = false;
    }

    pub(super) fn qr_from_file(&mut self) {
        let Some(path) = dialogs::abrir_arquivo(
            tr!("Escolha a imagem do QR Code", "Choose the QR code image"),
            dialogs::Filtro {
                nome: tr!("Imagens", "Images"),
                extensoes: &["png", "jpg", "jpeg", "bmp", "gif", "webp"],
            },
            None,
        ) else {
            return;
        };
        match image::open(&path) {
            Ok(img) => self.import_qr_image(img),
            Err(_) => error_box(tr!(
                "Não consegui abrir essa imagem.",
                "Could not open this image."
            )),
        }
    }

    pub(super) fn qr_from_clipboard(&mut self) {
        let img = arboard::Clipboard::new()
            .ok()
            .and_then(|mut c| c.get_image().ok())
            .and_then(|data| {
                image::RgbaImage::from_raw(
                    data.width as u32,
                    data.height as u32,
                    data.bytes.into_owned(),
                )
                .map(image::DynamicImage::ImageRgba8)
            });
        match img {
            Some(img) => self.import_qr_image(img),
            None => {
                dialogs::mensagem(
                    tr!(
                        "Não há imagem na área de transferência.\n\
                         Copie a imagem do QR Code e tente de novo.",
                        "There is no image on the clipboard.\n\
                         Copy the QR code image and try again."
                    ),
                    dialogs::Nivel::Info,
                );
            }
        }
    }

    pub(super) fn tela_editor(&mut self, ui: &mut egui::Ui) {
        let dark = self.dark;
        let mut salvar = false;
        let mut cancelar = false;
        let mut abrir_qr = false;
        let Some(ed) = self.editor.as_mut() else {
            return;
        };

        ui.label(
            egui::RichText::new(if ed.nova {
                tr!("Nova conta", "New account")
            } else {
                tr!("Editar conta", "Edit account")
            })
            .size(18.0)
            .strong(),
        );
        ui.add_space(4.0);

        egui::ScrollArea::vertical()
            .max_height((ui.available_height() - 60.0).max(120.0))
            .auto_shrink([false, true])
            .show(ui, |ui| {
                rotulo(ui, tr!("NOME", "NAME"), dark);
                ui.add(
                    egui::TextEdit::singleline(&mut ed.conta.nome)
                        .hint_text(tr!("ex.: Trabalho, Cliente X", "e.g. Work, Client X"))
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(4.0);

                rotulo(
                    ui,
                    tr!(
                        "ARQUIVO DE CONFIGURAÇÃO (.OVPN)",
                        "CONFIGURATION FILE (.OVPN)"
                    ),
                    dark,
                );
                ui.horizontal(|ui| {
                    let mut nome_arquivo = ed.conta.arquivo();
                    if nome_arquivo.is_empty() {
                        nome_arquivo = tr!("nenhum arquivo selecionado", "no file selected").into();
                    }
                    let largura = ui.available_width() - 118.0;
                    ui.add_enabled(
                        false,
                        egui::TextEdit::singleline(&mut nome_arquivo).desired_width(largura),
                    );
                    if ui.button(tr!("Procurar...", "Browse...")).clicked() {
                        let inicio = PathBuf::from(&ed.conta.config)
                            .parent()
                            .map(PathBuf::from)
                            .filter(|p| p.is_dir())
                            .or_else(|| {
                                platform::openvpn::pastas_de_config()
                                    .into_iter()
                                    .find(|d| d.is_dir())
                            });
                        let escolhido = dialogs::abrir_arquivo(
                            tr!("Escolha o arquivo .ovpn", "Choose the .ovpn file"),
                            dialogs::Filtro {
                                nome: tr!("Config OpenVPN", "OpenVPN config"),
                                extensoes: &["ovpn"],
                            },
                            inicio,
                        );
                        if let Some(p) = escolhido {
                            ed.conta.config = p.to_string_lossy().into_owned();
                            ajustar_autenticacao(&mut ed.conta);
                            if ed.conta.nome.trim().is_empty() {
                                if let Some(stem) = p.file_stem() {
                                    ed.conta.nome = stem.to_string_lossy().into_owned();
                                }
                            }
                        }
                    }
                });
                ui.add_space(4.0);

                if ed.conta.autenticacao.usa_usuario() {
                    rotulo(ui, tr!("USUÁRIO", "USERNAME"), dark);
                    ui.add(
                        egui::TextEdit::singleline(&mut ed.conta.usuario)
                            .desired_width(f32::INFINITY),
                    );
                    ui.add_space(4.0);
                }

                rotulo(ui, tr!("AUTENTICAÇÃO", "AUTHENTICATION"), dark);
                ui.horizontal_wrapped(|ui| {
                    for a in Autenticacao::TODAS {
                        ui.radio_value(&mut ed.conta.autenticacao, a, a.rotulo());
                    }
                });
                ui.add_space(4.0);

                if ed.conta.autenticacao.usa_senha() {
                    rotulo(ui, tr!("SENHA", "PASSWORD"), dark);
                    ui.horizontal(|ui| {
                        let largura = ui.available_width() - 98.0;
                        ui.add(
                            egui::TextEdit::singleline(&mut ed.conta.senha)
                                .password(!ed.mostrar_senha)
                                .desired_width(largura),
                        );
                        ui.checkbox(&mut ed.mostrar_senha, tr!("mostrar", "show"));
                    });
                    ui.add_space(4.0);
                }

                if ed.conta.autenticacao.usa_token() {
                    ui.horizontal(|ui| {
                        rotulo(
                            ui,
                            tr!("SEED DO GOOGLE AUTHENTICATOR", "GOOGLE AUTHENTICATOR SEED"),
                            dark,
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .button(
                                    egui::RichText::new(tr!(
                                        "Importar QR Code...",
                                        "Import QR code..."
                                    ))
                                    .small(),
                                )
                                .clicked()
                            {
                                abrir_qr = true;
                            }
                        });
                    });
                    ui.horizontal(|ui| {
                        let largura = ui.available_width() - 98.0;
                        ui.add(
                            egui::TextEdit::singleline(&mut ed.conta.seed)
                                .password(!ed.mostrar_seed)
                                .desired_width(largura),
                        );
                        ui.checkbox(&mut ed.mostrar_seed, tr!("mostrar", "show"));
                    });
                    // token preview: compare with the phone before saving
                    if let Some(seed) = totp::normalize_seed(&ed.conta.seed) {
                        if let Some(token) = totp::totp_now(&seed) {
                            ui.label(
                                egui::RichText::new(trf!(
                                    "Token atual: {} {}   ·   muda em {}s",
                                    "Current token: {} {}   ·   changes in {}s",
                                    &token[..3],
                                    &token[3..],
                                    totp::seconds_remaining()
                                ))
                                .color(label_color(dark)),
                            );
                        }
                    }
                }

                if let Some(erro) = &ed.erro {
                    ui.add_space(4.0);
                    ui.colored_label(egui::Color32::from_rgb(0xdc, 0x26, 0x26), erro);
                }
            });

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let btn = egui::Button::new(
                egui::RichText::new(tr!("Salvar", "Save")).color(egui::Color32::WHITE),
            )
            .fill(ACCENT);
            if ui.add(btn).clicked() {
                salvar = true;
            }
            if ui.button(tr!("Cancelar", "Cancel")).clicked() {
                cancelar = true;
            }
        });

        if abrir_qr {
            self.qr_open = true;
        }
        if cancelar {
            self.editor = None;
            self.qr_open = false;
            return;
        }
        if salvar {
            let Some(ed) = self.editor.as_mut() else {
                return;
            };
            let mut c = ed.conta.clone();
            c.nome = c.nome.trim().to_string();
            c.usuario = c.usuario.trim().to_string();
            if !c.autenticacao.usa_usuario() {
                c.usuario.clear();
            }
            if let Some(seed) = totp::normalize_seed(&c.seed) {
                c.seed = seed;
            }
            if !c.autenticacao.usa_token() {
                c.seed.clear();
            }
            if !c.autenticacao.usa_senha() {
                c.senha.clear();
            }
            match c.validar() {
                Ok(()) => {
                    let msg = if ed.nova {
                        tr!("Conta criada.", "Account created.")
                    } else {
                        tr!("Conta atualizada.", "Account updated.")
                    };
                    state::log(c.nome_exibicao(), msg);
                    engine::get().salvar_conta(c);
                    self.editor = None;
                    self.qr_open = false;
                    apply_tray_state();
                }
                Err(e) => ed.erro = Some(e),
            }
        }
    }

    pub(super) fn janela_qr(&mut self, ctx: &egui::Context) {
        let mut open_flag = true;
        let mut do_file = false;
        let mut do_clip = false;
        egui::Window::new(tr!("Importar QR Code", "Import QR code"))
            .id(egui::Id::new("janela_qr"))
            .open(&mut open_flag)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(tr!(
                    "Quando o suporte cadastra seu token, você recebe um QR Code —\n\
                     o mesmo que é escaneado no app Google Authenticator do celular.\n\
                     Esse QR Code contém o usuário e a seed:\n\n\
                     1. Salve a imagem do QR Code no computador (ou copie com\n\
                        PrintScreen / Ferramenta de Captura);\n\
                     2. Use um dos botões abaixo;\n\
                     3. Confira os campos preenchidos e clique em Salvar.\n\n\
                     Se você não tem o QR Code, peça ao suporte o recadastramento.",
                    "When support enrolls your token, you receive a QR code —\n\
                     the same one scanned with the Google Authenticator phone app.\n\
                     That QR code contains the username and the seed:\n\n\
                     1. Save the QR code image on the computer (or copy it with\n\
                        PrintScreen / Snipping Tool);\n\
                     2. Use one of the buttons below;\n\
                     3. Check the filled-in fields and click Save.\n\n\
                     If you don't have the QR code, ask support to enroll you again."
                ));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let pick = egui::Button::new(
                        egui::RichText::new(tr!("Escolher imagem...", "Choose image..."))
                            .color(egui::Color32::WHITE),
                    )
                    .fill(ACCENT);
                    if ui.add(pick).clicked() {
                        do_file = true;
                    }
                    if ui
                        .button(tr!("Colar imagem copiada", "Paste copied image"))
                        .clicked()
                    {
                        do_clip = true;
                    }
                });
            });
        self.qr_open = open_flag;
        if do_file {
            self.qr_from_file();
        }
        if do_clip {
            self.qr_from_clipboard();
        }
    }
}
