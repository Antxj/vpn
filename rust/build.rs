// Embute icone, manifesto UAC e versao no .exe via app.rc, e o instalador
// oficial do OpenVPN (assets/openvpn.msi) no binario.
// Com o toolchain GNU requer windres no PATH (vem no MinGW-w64).
//
// VPN_DEV_NOUAC=1 usa o manifesto asInvoker (sem pedir admin) -
// apenas para desenvolvimento/screenshots; a conexao real exige admin.
//
// O MSI NAO fica no repositorio (5,6 MB). O build-release.ps1 baixa e
// confere o SHA256 antes de compilar; sem o arquivo o binario sai sem o
// instalador embutido (o app cai no aviso com link de download).

use std::path::{Path, PathBuf};

/// Versao do MSI esperada em assets/openvpn.msi (mantida em sincronia com
/// o build-release.ps1, que baixa e confere o hash).
const MSI_VERSION: &str = "2.7.6-I001";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=app-dev.rc");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=assets/app.manifest");
    println!("cargo:rerun-if-changed=assets/app-dev.manifest");
    println!("cargo:rerun-if-changed=assets/openvpn.msi");
    println!("cargo:rerun-if-env-changed=VPN_DEV_NOUAC");

    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        let rc = if std::env::var_os("VPN_DEV_NOUAC").is_some() {
            "app-dev.rc"
        } else {
            "app.rc"
        };
        embed_resource::compile(rc, embed_resource::NONE);
    }

    // include_bytes! precisa de um caminho que sempre exista: quando o MSI
    // nao foi baixado, gera um arquivo vazio (o app detecta em tempo de
    // execucao e mantem o comportamento antigo).
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let embedded = out_dir.join("openvpn.msi");
    let source = Path::new("assets/openvpn.msi");
    if source.exists() {
        std::fs::copy(source, &embedded).expect("falha ao copiar o MSI para OUT_DIR");
    } else {
        std::fs::write(&embedded, b"").expect("falha ao criar o MSI vazio");
        println!(
            "cargo:warning=assets/openvpn.msi ausente: binario sem instalador \
             embutido (use build-release.ps1 para o release oficial)"
        );
    }
    println!("cargo:rustc-env=VPN_MSI={}", embedded.display());
    println!("cargo:rustc-env=VPN_MSI_VERSION={MSI_VERSION}");
}
