//! "Start with the system" on Linux: an XDG autostart entry
//! (~/.config/autostart/vpn.desktop) pointing to this executable with
//! ARG_STARTUP. Works on GNOME, KDE, Xfce, Cinnamon, MATE...

use std::path::PathBuf;

/// Command line argument that marks a start made by the system at logon.
pub const ARG_STARTUP: &str = "--startup";

fn pasta() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| super::paths::pasta_home().join(".config"))
        .join("autostart")
}

/// A test instance (VPN_INSTANCE) uses its own file, so tests never touch
/// the everyday app's entry.
fn arquivo() -> PathBuf {
    let nome = match std::env::var("VPN_INSTANCE") {
        Ok(s) if !s.is_empty() => format!("vpn-{s}.desktop"),
        _ => "vpn.desktop".into(),
    };
    pasta().join(nome)
}

/// Program to start: the AppImage file itself when running from one
/// (current_exe would be a temporary mount point), otherwise this executable.
fn programa() -> Option<String> {
    if let Some(appimage) = std::env::var_os("APPIMAGE") {
        return Some(appimage.to_string_lossy().into_owned());
    }
    Some(std::env::current_exe().ok()?.to_string_lossy().into_owned())
}

/// Quotes a path for the Exec key of a .desktop file.
fn aspas(s: &str) -> String {
    let mut saida = String::from("\"");
    for c in s.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            saida.push('\\');
        }
        saida.push(c);
    }
    saida.push('"');
    saida
}

fn conteudo(programa: &str) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=VPN\nComment=OpenVPN with automatic TOTP token\n\
         Exec={} {ARG_STARTUP}\nIcon=network-vpn\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
        aspas(programa)
    )
}

/// The app starts with the system.
pub fn ativo() -> bool {
    arquivo().exists()
}

/// Turns "start with the system" on or off.
pub fn definir(ligar: bool) -> Result<(), String> {
    let arquivo = arquivo();
    if !ligar {
        return match std::fs::remove_file(&arquivo) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        };
    }
    let programa = programa().ok_or_else(|| "current_exe".to_string())?;
    std::fs::create_dir_all(pasta()).map_err(|e| e.to_string())?;
    std::fs::write(&arquivo, conteudo(&programa)).map_err(|e| e.to_string())
}

/// If the app was moved (or updated to another path) while "start with the
/// system" is on, points the entry to the current location.
pub fn corrigir_caminho() {
    let Some(programa) = programa() else { return };
    if let Ok(atual) = std::fs::read_to_string(arquivo()) {
        if atual != conteudo(&programa) {
            let _ = definir(true);
        }
    }
}

/// This start was made by the system at logon.
pub fn iniciado_pelo_windows() -> bool {
    std::env::args().any(|a| a == ARG_STARTUP)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entrada_de_autostart() {
        let c = conteudo("/home/maria/Apps/VPN $teste.AppImage");
        assert!(
            c.contains("Exec=\"/home/maria/Apps/VPN \\$teste.AppImage\" --startup"),
            "{c}"
        );
        assert!(c.starts_with("[Desktop Entry]\n"));
    }
}
