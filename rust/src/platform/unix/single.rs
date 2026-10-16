//! Single instance on Linux and macOS: a Unix socket in the user's runtime
//! directory (Linux: $XDG_RUNTIME_DIR; macOS: the user's private temporary
//! folder).
//! The first instance listens on it; a second one connects, asks the first
//! to show itself and exits.

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Mutex;

static OUVINTE: Mutex<Option<UnixListener>> = Mutex::new(None);

/// $XDG_RUNTIME_DIR/vpn[-instance].sock (only this user can reach it).
/// VPN_INSTANCE (tests only) separates a test instance from the everyday app.
fn caminho() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join(format!("vpn-{}", unsafe { libc::getuid() })));
    let _ = std::fs::create_dir_all(&dir);
    let nome = match std::env::var("VPN_INSTANCE") {
        Ok(s) if !s.is_empty() => format!("vpn-{s}.sock"),
        _ => "vpn.sock".to_string(),
    };
    dir.join(nome)
}

/// Returns true if this is the first instance (and keeps listening for the
/// rest of the process). If another one exists, signals it and returns false.
pub fn acquire_or_signal() -> bool {
    let caminho = caminho();
    match UnixListener::bind(&caminho) {
        Ok(l) => {
            *OUVINTE.lock().unwrap() = Some(l);
            true
        }
        Err(_) => {
            if let Ok(mut s) = UnixStream::connect(&caminho) {
                let _ = s.write_all(b"mostrar\n");
                return false;
            }
            // nobody answers: left behind by a run that crashed
            let _ = std::fs::remove_file(&caminho);
            match UnixListener::bind(&caminho) {
                Ok(l) => {
                    *OUVINTE.lock().unwrap() = Some(l);
                    true
                }
                // could not take over: run anyway rather than not opening
                Err(_) => true,
            }
        }
    }
}

/// Thread that shows the main window whenever a second instance asks.
pub fn spawn_show_listener() {
    let Some(ouvinte) = OUVINTE.lock().unwrap().take() else {
        return;
    };
    std::thread::spawn(move || {
        for conexao in ouvinte.incoming().flatten() {
            let mut buf = [0u8; 16];
            let mut conexao = conexao;
            let _ = conexao.read(&mut buf);
            crate::show_main_window();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segunda_instancia_avisa_a_primeira() {
        std::env::set_var(
            "VPN_INSTANCE",
            format!("teste-single-{}", std::process::id()),
        );
        assert!(acquire_or_signal());
        let ouvinte = OUVINTE.lock().unwrap().take().unwrap();
        ouvinte.set_nonblocking(false).unwrap();
        // a second instance connects and asks the first one to show itself
        assert!(!acquire_or_signal());
        let (mut c, _) = ouvinte.accept().unwrap();
        let mut texto = String::new();
        c.read_to_string(&mut texto).unwrap();
        assert_eq!(texto, "mostrar\n");
        drop(ouvinte);
        let _ = std::fs::remove_file(caminho());
    }
}
