// Embeds the icon, UAC manifest and version into the .exe via app.rc, and the
// official OpenVPN installer (assets/openvpn.msi) into the binary.
// With the GNU toolchain it requires windres on the PATH (ships with MinGW-w64).
//
// The MSI is NOT in the repository (5.6 MB). build-release.ps1 downloads it
// and verifies the SHA256 before building; without the file the binary has
// no embedded installer (the app falls back to the notice with a download link).

use std::path::{Path, PathBuf};

/// MSI version expected in assets/openvpn.msi (kept in sync with
/// build-release.ps1, which downloads it and verifies the hash).
const MSI_VERSION: &str = "2.7.6-I001";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=assets/app.manifest");
    println!("cargo:rerun-if-changed=assets/openvpn.msi");

    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        embed_resource::compile("app.rc", embed_resource::NONE);
    }

    // include_bytes! needs a path that always exists: when the MSI was not
    // downloaded, an empty file is generated (the app detects it at run time
    // and keeps the old behavior).
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let embedded = out_dir.join("openvpn.msi");
    let source = Path::new("assets/openvpn.msi");
    if source.exists() {
        std::fs::copy(source, &embedded).expect("failed to copy the MSI to OUT_DIR");
    } else {
        std::fs::write(&embedded, b"").expect("failed to create the empty MSI");
        println!(
            "cargo:warning=assets/openvpn.msi missing: binary without the embedded \
             installer (use build-release.ps1 for the official release)"
        );
    }
    println!("cargo:rustc-env=VPN_MSI={}", embedded.display());
    println!("cargo:rustc-env=VPN_MSI_VERSION={MSI_VERSION}");
}
