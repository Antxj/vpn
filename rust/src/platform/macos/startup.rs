//! "Start with the system" on macOS: a LaunchAgent of the user
//! (~/Library/LaunchAgents/io.github.antxj.vpn.plist) that opens this
//! executable with ARG_STARTUP at login. No administrator needed.

use std::path::PathBuf;

/// Command line argument that marks a start made by the system at login.
pub const ARG_STARTUP: &str = "--startup";

/// A test instance (VPN_INSTANCE) uses its own label and file, so tests
/// never touch the everyday app's entry.
fn rotulo() -> String {
    match std::env::var("VPN_INSTANCE") {
        Ok(s) if !s.is_empty() => format!("io.github.antxj.vpn-{s}"),
        _ => "io.github.antxj.vpn".into(),
    }
}

fn arquivo() -> PathBuf {
    super::paths::pasta_home()
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{}.plist", rotulo()))
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn conteudo(programa: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n\
         \x20   <key>Label</key>\n    <string>{}</string>\n\
         \x20   <key>ProgramArguments</key>\n    <array>\n\
         \x20       <string>{}</string>\n        <string>{ARG_STARTUP}</string>\n\
         \x20   </array>\n\
         \x20   <key>RunAtLoad</key>\n    <true/>\n\
         \x20   <key>ProcessType</key>\n    <string>Interactive</string>\n\
         </dict>\n</plist>\n",
        xml(&rotulo()),
        xml(programa)
    )
}

fn programa() -> Option<String> {
    Some(std::env::current_exe().ok()?.to_string_lossy().into_owned())
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
    if let Some(pasta) = arquivo.parent() {
        std::fs::create_dir_all(pasta).map_err(|e| e.to_string())?;
    }
    std::fs::write(&arquivo, conteudo(&programa)).map_err(|e| e.to_string())
}

/// If the app was moved while "start with the system" is on, points the
/// entry to the current location.
pub fn corrigir_caminho() {
    let Some(programa) = programa() else { return };
    if let Ok(atual) = std::fs::read_to_string(arquivo()) {
        if atual != conteudo(&programa) {
            let _ = definir(true);
        }
    }
}

/// This start was made by the system at login.
pub fn iniciado_pelo_windows() -> bool {
    std::env::args().any(|a| a == ARG_STARTUP)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_agent() {
        let c = conteudo("/Applications/VPN & <teste>.app/Contents/MacOS/VPN");
        assert!(
            c.contains(
                "<string>/Applications/VPN &amp; &lt;teste&gt;.app/Contents/MacOS/VPN</string>"
            ),
            "{c}"
        );
        assert!(c.contains("<string>--startup</string>"));
        assert!(c.contains("<key>RunAtLoad</key>\n    <true/>"));
    }
}
