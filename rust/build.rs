// Windows: embeds the icon, UAC manifest and version into the .exe via
// app.rc, and the official OpenVPN installer (assets/openvpn.msi) into the
// binary. With the GNU toolchain it requires windres on the PATH (ships with
// MinGW-w64).
//
// macOS: embeds the OpenVPN built for the app (assets/openvpn-macos-<arch>)
// and OpenVPN's DNS script for macOS (assets/dns-updown-macos.sh); the
// one-time authorization installs both, root-owned.
//
// These files are NOT in the repository. build-release.ps1 downloads the MSI
// and build-release-macos.sh builds OpenVPN, both from sources with a pinned
// SHA-256; without them the binary has nothing embedded (the app then shows
// the notice with a download link).

use std::path::{Path, PathBuf};

/// MSI version expected in assets/openvpn.msi (kept in sync with
/// build-release.ps1, which downloads it and verifies the hash).
const MSI_VERSION: &str = "2.7.6-I001";

/// Copies `origem` to OUT_DIR/`nome` (include_bytes! needs a path that always
/// exists: when the file is missing, an empty one is written).
fn embutir(origem: &str, nome: &str, aviso: &str) -> PathBuf {
    println!("cargo:rerun-if-changed={origem}");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let destino = out_dir.join(nome);
    if Path::new(origem).exists() {
        std::fs::copy(origem, &destino).expect("failed to copy to OUT_DIR");
    } else {
        std::fs::write(&destino, b"").expect("failed to create the empty file");
        println!("cargo:warning={origem} missing: {aviso}");
    }
    destino
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let sistema = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    if sistema == "windows" {
        println!("cargo:rerun-if-changed=app.rc");
        println!("cargo:rerun-if-changed=assets/icon.ico");
        println!("cargo:rerun-if-changed=assets/app.manifest");
        embed_resource::compile("app.rc", embed_resource::NONE);
        let msi = embutir(
            "assets/openvpn.msi",
            "openvpn.msi",
            "binary without the embedded installer (use build-release.ps1 for the official release)",
        );
        println!("cargo:rustc-env=VPN_MSI={}", msi.display());
        println!("cargo:rustc-env=VPN_MSI_VERSION={MSI_VERSION}");
    }

    if sistema == "macos" {
        let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
        let aviso = "binary without OpenVPN (use build-release-macos.sh for the official release)";
        let openvpn = embutir(
            &format!("assets/openvpn-macos-{arch}"),
            "openvpn-macos",
            aviso,
        );
        let dns = embutir("assets/dns-updown-macos.sh", "dns-updown-macos.sh", aviso);
        println!("cargo:rustc-env=VPN_OPENVPN_MACOS={}", openvpn.display());
        println!("cargo:rustc-env=VPN_DNS_MACOS={}", dns.display());
    }
}
