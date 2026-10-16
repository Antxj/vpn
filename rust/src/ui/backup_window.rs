//! Export and import window of the account backup.

use super::*;

/// File filter of the account backup.
pub(super) fn filtro_backup() -> dialogs::Filtro<'static> {
    dialogs::Filtro {
        nome: tr!("Backup do VPN", "VPN backup"),
        extensoes: &["vpnbackup"],
    }
}

/// "1 conta", "3 contas" (in the current language).
pub(super) fn contas_texto(n: usize) -> String {
    if n == 1 {
        tr!("1 conta", "1 account").into()
    } else {
        trf!("{n} contas", "{n} accounts")
    }
}

/// The next step of the backup window (save, open or import).
pub(super) fn executar_backup(jb: &mut JanelaBackup, m: &engine::Motor) -> Result<(), String> {
    if jb.exportar {
        if jb.senha.chars().count() < backup::SENHA_MINIMA {
            return Err(trf!(
                "A senha precisa ter pelo menos {} caracteres.",
                "The password needs at least {} characters.",
                backup::SENHA_MINIMA
            ));
        }
        if jb.senha != jb.confirmar {
            return Err(tr!(
                "As duas senhas não são iguais.",
                "The two passwords do not match."
            )
            .into());
        }
        let data = state::data_e_hora_local();
        let Some(destino) = dialogs::salvar_arquivo(
            tr!("Salvar o backup das contas", "Save the account backup"),
            filtro_backup(),
            &format!("VPN-{}.vpnbackup", &data[..10]),
        ) else {
            return Ok(());
        };
        let r = backup::exportar(&m.contas(), &jb.senha, &destino)?;
        jb.senha.clear();
        jb.confirmar.clear();
        let mut msg = trf!(
            "Backup com {} salvo em {}.\n\nGuarde a senha em lugar seguro: sem ela não \
             dá para abrir o arquivo.",
            "Backup with {} saved to {}.\n\nKeep the password somewhere safe: the file \
             cannot be opened without it.",
            contas_texto(r.contas),
            destino.display()
        );
        if !r.nao_incluidos.is_empty() {
            msg.push_str(&trf!(
                "\n\nNão foram incluídos (fora da pasta do .ovpn): {}",
                "\n\nNot included (outside the .ovpn folder): {}",
                r.nao_incluidos.join(", ")
            ));
        }
        jb.concluido = Some(msg);
    } else if let Some(contas) = jb.contas.take() {
        let prontas = match backup::instalar(&contas, &settings::app_dir().join("configs")) {
            Ok(p) => p,
            Err(e) => {
                jb.contas = Some(contas);
                return Err(e);
            }
        };
        for c in &prontas {
            m.salvar_conta(c.clone());
        }
        jb.concluido = Some(trf!(
            "Importado: {}.",
            "Imported: {}.",
            contas_texto(prontas.len())
        ));
    } else {
        jb.contas = Some(backup::abrir(&jb.arquivo, &jb.senha)?);
        jb.senha.clear();
    }
    Ok(())
}

impl App {
    /// Export / import of the account backup (password-protected file).
    pub(super) fn janela_backup(&mut self, ctx: &egui::Context) {
        let dark = self.dark;
        let Some(jb) = self.backup.as_mut() else {
            return;
        };
        let m = engine::get();
        let mut aberta = true;
        let mut fechar = false;
        let titulo = if jb.exportar {
            tr!("Exportar contas", "Export accounts")
        } else {
            tr!("Importar contas", "Import accounts")
        };
        let campo_senha = |ui: &mut egui::Ui, nome: &str, valor: &mut String, mostrar: bool| {
            rotulo(ui, nome, dark);
            ui.add(
                egui::TextEdit::singleline(valor)
                    .password(!mostrar)
                    .desired_width(f32::INFINITY),
            )
        };
        egui::Window::new(titulo)
            .id(egui::Id::new("janela_backup"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut aberta)
            .show(ctx, |ui| {
                ui.set_max_width(380.0);
                if let Some(msg) = &jb.concluido {
                    ui.label(msg);
                    ui.add_space(8.0);
                    if ui.button(tr!("Fechar", "Close")).clicked() {
                        fechar = true;
                    }
                    return;
                }
                let mut agir = false;
                if jb.exportar {
                    let n = contas_texto(m.contas().len());
                    ui.label(trf!(
                        "Cria um arquivo com {n}, protegido pela senha abaixo. Guarde \
                         essa senha: sem ela o arquivo não abre.",
                        "Creates a file with {n}, protected by the password below. Keep \
                         this password: the file does not open without it."
                    ));
                    ui.add_space(6.0);
                    let r = campo_senha(
                        ui,
                        tr!("SENHA DO BACKUP", "BACKUP PASSWORD"),
                        &mut jb.senha,
                        jb.mostrar,
                    );
                    if jb.senha.is_empty() && jb.confirmar.is_empty() && jb.erro.is_none() {
                        r.request_focus();
                    }
                    ui.add_space(4.0);
                    let r = campo_senha(
                        ui,
                        tr!("REPITA A SENHA", "REPEAT THE PASSWORD"),
                        &mut jb.confirmar,
                        jb.mostrar,
                    );
                    agir |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                } else if let Some(contas) = &jb.contas {
                    ui.label(trf!(
                        "Este backup tem {}:",
                        "This backup has {}:",
                        contas_texto(contas.len())
                    ));
                    ui.add_space(4.0);
                    let existentes = m.contas();
                    for c in contas {
                        let mut linha = format!("•  {}", c.conta.nome_exibicao());
                        if existentes.iter().any(|e| e.id == c.conta.id) {
                            linha.push_str(tr!(
                                "  (atualiza a que já existe)",
                                "  (updates the existing one)"
                            ));
                        }
                        ui.label(linha);
                    }
                } else {
                    ui.label(trf!(
                        "Arquivo: {}",
                        "File: {}",
                        jb.arquivo
                            .file_name()
                            .map(|n| n.to_string_lossy())
                            .unwrap_or_default()
                    ));
                    ui.add_space(6.0);
                    let r = campo_senha(
                        ui,
                        tr!("SENHA DO BACKUP", "BACKUP PASSWORD"),
                        &mut jb.senha,
                        jb.mostrar,
                    );
                    if jb.senha.is_empty() && jb.erro.is_none() {
                        r.request_focus();
                    }
                    agir |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                }
                if jb.contas.is_none() {
                    ui.checkbox(&mut jb.mostrar, tr!("mostrar senha", "show password"));
                }
                if let Some(e) = &jb.erro {
                    ui.add_space(4.0);
                    ui.colored_label(egui::Color32::from_rgb(0xdc, 0x26, 0x26), e);
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let acao = if jb.exportar {
                        tr!("Salvar backup...", "Save backup...")
                    } else if jb.contas.is_some() {
                        tr!("Importar", "Import")
                    } else {
                        tr!("Abrir", "Open")
                    };
                    let btn =
                        egui::Button::new(egui::RichText::new(acao).color(egui::Color32::WHITE))
                            .fill(ACCENT);
                    agir |= ui.add(btn).clicked();
                    if ui.button(tr!("Cancelar", "Cancel")).clicked() {
                        fechar = true;
                    }
                });
                if agir {
                    jb.erro = None;
                    if let Err(e) = executar_backup(jb, m) {
                        jb.erro = Some(e);
                    }
                }
            });
        if !aberta || fechar {
            self.backup = None;
        }
    }
}
