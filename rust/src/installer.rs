//! Instalacao automatica do OpenVPN Community a partir do MSI oficial
//! embutido no executavel.
//!
//! O MSI e o pacote assinado pela OpenVPN Inc., redistribuido sem
//! modificacao (ver THIRD-PARTY-LICENSES.txt). A instalacao e silenciosa e
//! escolhe um conjunto minimo de componentes: nucleo, servico e driver
//! TAP-Windows6 - de proposito SEM a interface grafica do OpenVPN, que
//! colocaria um segundo icone de VPN na bandeja e confundiria o usuario.

use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::Sender;

/// MSI embutido (vazio quando a compilacao nao encontrou assets/openvpn.msi).
static MSI: &[u8] = include_bytes!(env!("VPN_MSI"));
pub const MSI_VERSION: &str = env!("VPN_MSI_VERSION");

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Componentes instalados (nomes conferidos na tabela Feature do MSI).
const FEATURES: &str = "ADDLOCAL=OpenVPN,OpenVPN.Service,Drivers,Drivers.TAPWindows6";
/// msiexec: sucesso, porem o Windows pede reinicializacao.
const ERROR_SUCCESS_REBOOT_REQUIRED: i32 = 3010;

pub enum Event {
    /// Terminou: Ok(reiniciar_recomendado) ou Err(mensagem).
    Done(Result<bool, String>),
}

pub fn is_available() -> bool {
    !MSI.is_empty()
}

fn log_path() -> PathBuf {
    crate::dpapi::app_dir().join("openvpn-install.log")
}

/// Instala o OpenVPN em segundo plano. Requer privilegio de administrador
/// (o app inteiro ja roda elevado pelo manifesto).
pub fn install_in_background(tx: Sender<Event>, ctx: eframe::egui::Context) {
    std::thread::spawn(move || {
        let result = install();
        let _ = tx.send(Event::Done(result));
        ctx.request_repaint();
    });
}

fn install() -> Result<bool, String> {
    if !is_available() {
        return Err(tr!(
            "Esta compilação não tem o instalador embutido.",
            "This build does not include the embedded installer."
        )
        .into());
    }

    let msi_path = std::env::temp_dir().join(format!("vpn-openvpn-{MSI_VERSION}.msi"));
    std::fs::write(&msi_path, MSI)
        .map_err(|e| trf!("Não consegui gravar o instalador em disco: {e}", "Could not write the installer to disk: {e}"))?;

    let log = log_path();
    let _ = std::fs::create_dir_all(crate::dpapi::app_dir());

    let status = Command::new("msiexec.exe")
        .arg("/i")
        .arg(&msi_path)
        .args(["/qn", "/norestart", FEATURES, "/l*v"])
        .arg(&log)
        .creation_flags(CREATE_NO_WINDOW)
        .status();

    let _ = std::fs::remove_file(&msi_path);

    let status = status.map_err(|e| trf!("Não consegui executar o msiexec: {e}", "Could not run msiexec: {e}"))?;
    match status.code() {
        Some(0) => Ok(false),
        Some(ERROR_SUCCESS_REBOOT_REQUIRED) => Ok(true),
        Some(1602) => Err(tr!("A instalação foi cancelada.", "The installation was cancelled.").into()),
        Some(code) => Err(trf!(
            "A instalação falhou (código {code}).\nDetalhes em:\n{}",
            "The installation failed (code {code}).\nDetails in:\n{}",
            log.display()
        )),
        None => Err(tr!("A instalação foi interrompida.", "The installation was interrupted.").into()),
    }
}

// creation_flags vem da extensao de Command no Windows
use std::os::windows::process::CommandExt;

#[cfg(test)]
mod tests {
    use super::*;

    /// O MSI embutido precisa ser um pacote MSI de verdade (ou estar
    /// ausente, no build de desenvolvimento) - nunca lixo.
    #[test]
    fn msi_embutido_e_valido_ou_ausente() {
        if MSI.is_empty() {
            return; // build sem o instalador: aceito em desenvolvimento
        }
        // MSI e um arquivo OLE2: assinatura D0 CF 11 E0 A1 B1 1A E1
        assert_eq!(
            &MSI[..8],
            &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1],
            "o arquivo embutido nao parece um MSI"
        );
        assert!(MSI.len() > 1_000_000, "MSI embutido pequeno demais");
        assert!(!MSI_VERSION.is_empty());
    }

    #[test]
    fn features_cobrem_nucleo_e_driver() {
        // sem driver de rede o OpenVPN instala mas nao conecta
        assert!(FEATURES.contains("Drivers.TAPWindows6"));
        assert!(FEATURES.contains("OpenVPN.Service"));
        // a interface grafica do OpenVPN nao deve ser instalada
        assert!(!FEATURES.contains("OpenVPN.GUI"));
    }
}
