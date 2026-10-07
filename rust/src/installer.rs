//! Automatic installation of OpenVPN Community from the official MSI
//! embedded in the executable.
//!
//! The MSI is the package signed by OpenVPN Inc., redistributed without
//! modification (see THIRD-PARTY-LICENSES.txt). The installation is silent and
//! picks a minimal set of components: core, service and the TAP-Windows6
//! driver - deliberately WITHOUT the OpenVPN GUI, which would add a second
//! VPN icon to the tray and confuse the user.

use std::path::PathBuf;
use std::sync::mpsc::Sender;

/// Embedded MSI (empty when the build did not find assets/openvpn.msi).
static MSI: &[u8] = include_bytes!(env!("VPN_MSI"));
pub const MSI_VERSION: &str = env!("VPN_MSI_VERSION");

/// Installed components (names checked against the MSI Feature table).
const FEATURES: &str = "ADDLOCAL=OpenVPN,OpenVPN.Service,Drivers,Drivers.TAPWindows6";
/// msiexec: success, but Windows asks for a restart.
const ERROR_SUCCESS_REBOOT_REQUIRED: u32 = 3010;

pub enum Event {
    /// Finished: Ok(restart_recommended) or Err(message).
    Done(Result<bool, String>),
}

pub fn is_available() -> bool {
    !MSI.is_empty()
}

fn log_path() -> PathBuf {
    crate::dpapi::app_dir().join("openvpn-install.log")
}

/// Installs OpenVPN in the background. Windows asks for administrator
/// permission (UAC) at this moment.
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

    // the app runs as a regular user: Windows asks for administrator
    // permission (UAC) for the installation only
    let msiexec = PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()))
        .join("System32")
        .join("msiexec.exe");
    let params = format!(
        "/i {} /qn /norestart {FEATURES} /l*v {}",
        crate::service::argumento(&msi_path.to_string_lossy()),
        crate::service::argumento(&log.to_string_lossy())
    );
    let resultado = crate::elevate::executar_como_admin(&msiexec, &params);

    let _ = std::fs::remove_file(&msi_path);

    match resultado? {
        0 => Ok(false),
        ERROR_SUCCESS_REBOOT_REQUIRED => Ok(true),
        1602 => Err(tr!("A instalação foi cancelada.", "The installation was cancelled.").into()),
        code => Err(trf!(
            "A instalação falhou (código {code}).\nDetalhes em:\n{}",
            "The installation failed (code {code}).\nDetails in:\n{}",
            log.display()
        )),
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    /// The embedded MSI must be a real MSI package (or be absent, in the
    /// development build) - never garbage.
    #[test]
    fn msi_embutido_e_valido_ou_ausente() {
        if MSI.is_empty() {
            return; // build without the installer: accepted in development
        }
        // an MSI is an OLE2 file: signature D0 CF 11 E0 A1 B1 1A E1
        assert_eq!(
            &MSI[..8],
            &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1],
            "o arquivo embutido nao parece um MSI"
        );
        assert!(MSI.len() > 1_000_000, "embedded MSI too small");
        assert!(!MSI_VERSION.is_empty());
    }

    #[test]
    fn features_cobrem_nucleo_e_driver() {
        // without a network driver OpenVPN installs but does not connect
        assert!(FEATURES.contains("Drivers.TAPWindows6"));
        assert!(FEATURES.contains("OpenVPN.Service"));
        // the OpenVPN GUI must not be installed
        assert!(!FEATURES.contains("OpenVPN.GUI"));
    }
}
