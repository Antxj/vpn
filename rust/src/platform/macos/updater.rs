//! macOS side of the updater (the download and the checks are in
//! unix/updater.rs). The release asset is VPN-macos.zip with VPN.app: the
//! executable is taken from it and swapped like on the other systems; then
//! the bundle's Info.plist and icon are brought along and the bundle is
//! signed again (ad hoc), so it stays consistent.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

pub use crate::platform::unix::updater::{
    aguardar_processo, assinante, http_get, sha256_hex, tornar_executavel,
};

/// Name of the release asset that contains the app.
pub const ARQUIVO: &str = "VPN-macos.zip";
/// The executable inside the .zip is a universal binary (Intel and Apple Silicon).
pub const CABECALHO: &[u8] = &[0xca, 0xfe, 0xba, 0xbe];

/// Folder where the downloaded .zip was opened (until the swap is done).
static ABERTO: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Opens the downloaded .zip (with macOS's own ditto) and returns the
/// executable of the VPN.app inside it.
pub fn extrair(dados: Vec<u8>) -> Result<Vec<u8>, String> {
    let falha = |e: String| {
        trf!(
            "Não consegui abrir o {ARQUIVO} baixado: {e}",
            "Could not open the downloaded {ARQUIVO}: {e}"
        )
    };
    let pasta = std::env::temp_dir().join(format!("vpn-atualizacao-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&pasta);
    std::fs::create_dir_all(&pasta).map_err(|e| falha(e.to_string()))?;
    let zip = pasta.join(ARQUIVO);
    std::fs::write(&zip, &dados).map_err(|e| falha(e.to_string()))?;
    let ok = Command::new("/usr/bin/ditto")
        .arg("-x")
        .arg("-k")
        .arg(&zip)
        .arg(&pasta)
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        return Err(falha("ditto".into()));
    }
    let exe = std::fs::read(pasta.join("VPN.app/Contents/MacOS/VPN"))
        .map_err(|e| falha(e.to_string()))?;
    *ABERTO.lock().unwrap() = Some(pasta);
    Ok(exe)
}

/// After the executable was swapped: the old one (still running) goes away,
/// the new Info.plist and icon come in, and the bundle is signed again.
pub fn depois_de_trocar(exe: &Path) {
    let Some(pasta) = ABERTO.lock().unwrap().take() else {
        return;
    };
    // .../VPN.app/Contents/MacOS/VPN -> .../VPN.app/Contents
    if let Some(contents) = exe.parent().and_then(Path::parent) {
        let novo = pasta.join("VPN.app/Contents");
        if contents.ends_with("Contents") && contents.join("Info.plist").exists() {
            // a running executable can be deleted on macOS; inside the bundle
            // it would break the signature (update.rs keeps it as <exe>.antigo)
            let antigo = exe.with_file_name(format!(
                "{}.antigo",
                exe.file_name().unwrap_or_default().to_string_lossy()
            ));
            let _ = std::fs::remove_file(antigo);
            let _ = std::fs::copy(novo.join("Info.plist"), contents.join("Info.plist"));
            let _ = std::fs::copy(
                novo.join("Resources/VPN.icns"),
                contents.join("Resources/VPN.icns"),
            );
            if let Some(app) = contents.parent() {
                let _ = Command::new("/usr/bin/codesign")
                    .args(["--force", "--sign", "-"])
                    .arg(app)
                    .status();
            }
        }
    }
    let _ = std::fs::remove_dir_all(pasta);
}
