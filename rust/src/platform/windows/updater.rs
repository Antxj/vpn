//! Windows side of the updater: HTTPS download (WinHTTP, honors the system
//! proxy), SHA-256 (BCrypt), Authenticode signature (WinVerifyTrust) and
//! waiting for the previous process.

use crate::update::{ErroHttp, REPOSITORIO, VERSAO_ATUAL};
use std::ffi::c_void;
use std::path::Path;
use std::ptr;
use std::time::Duration;

/// Name of the release asset that contains the app.
pub const ARQUIVO: &str = "VPN.exe";
/// The downloaded file must start like an executable.
pub const CABECALHO: &[u8] = b"MZ";

/// Nothing to do: on Windows any file can be run.
pub fn tornar_executavel(_arquivo: &Path) {}

/// The downloaded file is the executable itself.
pub fn extrair(dados: Vec<u8>) -> Result<Vec<u8>, String> {
    Ok(dados)
}

/// Nothing besides the executable to update.
pub fn depois_de_trocar(_exe: &Path) {}

/// On the first start after an update: waits for the previous version to close
/// (it is still disconnecting the VPNs and holding the single instance).
pub fn aguardar_processo(pid: u32, limite: Duration) {
    const SYNCHRONIZE: u32 = 0x0010_0000;
    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(acesso: u32, herdar: i32, pid: u32) -> *mut c_void;
        fn WaitForSingleObject(h: *mut c_void, ms: u32) -> u32;
        fn CloseHandle(h: *mut c_void) -> i32;
    }
    unsafe {
        let h = OpenProcess(SYNCHRONIZE, 0, pid);
        if !h.is_null() {
            WaitForSingleObject(h, limite.as_millis() as u32);
            CloseHandle(h);
        }
    }
}

pub fn sha256_hex(dados: &[u8]) -> Option<String> {
    use windows_sys::Win32::Security::Cryptography::{BCryptHash, BCRYPT_SHA256_ALG_HANDLE};
    let mut saida = [0u8; 32];
    let status = unsafe {
        BCryptHash(
            BCRYPT_SHA256_ALG_HANDLE,
            ptr::null(),
            0,
            dados.as_ptr(),
            u32::try_from(dados.len()).ok()?,
            saida.as_mut_ptr(),
            saida.len() as u32,
        )
    };
    (status == 0).then(|| saida.iter().map(|b| format!("{b:02x}")).collect())
}

/// Publisher name, if the file has a valid Authenticode signature.
pub fn assinante(caminho: &Path) -> Option<String> {
    use windows_sys::Win32::Security::Cryptography::{
        CertGetNameStringW, CERT_NAME_SIMPLE_DISPLAY_TYPE,
    };
    use windows_sys::Win32::Security::WinTrust::*;

    let caminho_w = super::single::wide(&caminho.to_string_lossy());
    let mut arquivo = WINTRUST_FILE_INFO {
        cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: caminho_w.as_ptr(),
        hFile: ptr::null_mut(),
        pgKnownSubject: ptr::null_mut(),
    };
    let mut dados: WINTRUST_DATA = unsafe { std::mem::zeroed() };
    dados.cbStruct = std::mem::size_of::<WINTRUST_DATA>() as u32;
    dados.dwUIChoice = WTD_UI_NONE;
    dados.fdwRevocationChecks = WTD_REVOKE_WHOLECHAIN;
    dados.dwProvFlags = WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT;
    dados.dwUnionChoice = WTD_CHOICE_FILE;
    dados.Anonymous.pFile = &mut arquivo;
    dados.dwStateAction = WTD_STATEACTION_VERIFY;

    let mut acao = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    let sem_janela = -1isize as *mut c_void; // INVALID_HANDLE_VALUE: never shows UI
    let resultado =
        unsafe { WinVerifyTrust(sem_janela, &mut acao, &mut dados as *mut _ as *mut c_void) };

    let mut nome = None;
    if resultado == 0 {
        unsafe {
            let prov = WTHelperProvDataFromStateData(dados.hWVTStateData);
            let sgnr = if prov.is_null() {
                ptr::null_mut()
            } else {
                WTHelperGetProvSignerFromChain(prov, 0, 0, 0)
            };
            let cert = if sgnr.is_null() {
                ptr::null_mut()
            } else {
                WTHelperGetProvCertFromChain(sgnr, 0)
            };
            if !cert.is_null() && !(*cert).pCert.is_null() {
                let mut buf = [0u16; 256];
                let n = CertGetNameStringW(
                    (*cert).pCert,
                    CERT_NAME_SIMPLE_DISPLAY_TYPE,
                    0,
                    ptr::null(),
                    buf.as_mut_ptr(),
                    buf.len() as u32,
                );
                if n > 1 {
                    nome = Some(String::from_utf16_lossy(&buf[..n as usize - 1]));
                }
            }
        }
    }
    dados.dwStateAction = WTD_STATEACTION_CLOSE;
    unsafe {
        WinVerifyTrust(sem_janela, &mut acao, &mut dados as *mut _ as *mut c_void);
    }
    nome.filter(|n| !n.is_empty())
}

/// Closes the WinHTTP handle when it goes out of scope.
struct Handle(*mut c_void);
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { windows_sys::Win32::Networking::WinHttp::WinHttpCloseHandle(self.0) };
        }
    }
}

fn ultimo_erro() -> u32 {
    unsafe { windows_sys::Win32::Foundation::GetLastError() }
}

/// Simple GET (follows HTTPS redirects, like the one of the asset
/// downloads). `progresso` receives the bytes read so far.
pub fn http_get(
    url: &str,
    limite: usize,
    progresso: &mut dyn FnMut(u64),
) -> Result<Vec<u8>, ErroHttp> {
    use windows_sys::Win32::Networking::WinHttp::*;

    let url_w: Vec<u16> = url.encode_utf16().collect();
    let mut partes: URL_COMPONENTS = unsafe { std::mem::zeroed() };
    partes.dwStructSize = std::mem::size_of::<URL_COMPONENTS>() as u32;
    // -1: returns pointers into the URL itself
    partes.dwSchemeLength = u32::MAX;
    partes.dwHostNameLength = u32::MAX;
    partes.dwUrlPathLength = u32::MAX;
    partes.dwExtraInfoLength = u32::MAX;
    if unsafe { WinHttpCrackUrl(url_w.as_ptr(), url_w.len() as u32, 0, &mut partes) } == 0 {
        return Err(ErroHttp::Url);
    }
    let fatia = |p: *const u16, n: u32| -> Vec<u16> {
        if p.is_null() {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(p, n as usize) }.to_vec()
        }
    };
    let mut host = fatia(partes.lpszHostName, partes.dwHostNameLength);
    host.push(0);
    let mut caminho = fatia(partes.lpszUrlPath, partes.dwUrlPathLength);
    caminho.extend(fatia(partes.lpszExtraInfo, partes.dwExtraInfoLength));
    if caminho.is_empty() {
        caminho.push(b'/' as u16);
    }
    caminho.push(0);
    let https = partes.nScheme == WINHTTP_INTERNET_SCHEME_HTTPS;

    let agente = super::single::wide(&format!(
        "VPN/{VERSAO_ATUAL} (+https://github.com/{REPOSITORIO})"
    ));
    let sessao = Handle(unsafe {
        WinHttpOpen(
            agente.as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            ptr::null(),
            ptr::null(),
            0,
        )
    });
    if sessao.0.is_null() {
        return Err(ErroHttp::Rede(ultimo_erro()));
    }
    unsafe { WinHttpSetTimeouts(sessao.0, 15_000, 15_000, 30_000, 30_000) };

    let conexao = Handle(unsafe { WinHttpConnect(sessao.0, host.as_ptr(), partes.nPort, 0) });
    if conexao.0.is_null() {
        return Err(ErroHttp::Rede(ultimo_erro()));
    }
    let verbo = super::single::wide("GET");
    let pedido = Handle(unsafe {
        WinHttpOpenRequest(
            conexao.0,
            verbo.as_ptr(),
            caminho.as_ptr(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            if https { WINHTTP_FLAG_SECURE } else { 0 },
        )
    });
    if pedido.0.is_null() {
        return Err(ErroHttp::Rede(ultimo_erro()));
    }
    let cabecalhos = super::single::wide(
        "Accept: application/vnd.github+json, application/octet-stream\r\n\
         X-GitHub-Api-Version: 2022-11-28\r\n",
    );
    let ok = unsafe {
        WinHttpSendRequest(
            pedido.0,
            cabecalhos.as_ptr(),
            u32::MAX,
            ptr::null(),
            0,
            0,
            0,
        ) != 0
            && WinHttpReceiveResponse(pedido.0, ptr::null_mut()) != 0
    };
    if !ok {
        return Err(ErroHttp::Rede(ultimo_erro()));
    }

    let mut status: u32 = 0;
    let mut tam = std::mem::size_of::<u32>() as u32;
    unsafe {
        WinHttpQueryHeaders(
            pedido.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            ptr::null(),
            &mut status as *mut u32 as *mut c_void,
            &mut tam,
            ptr::null_mut(),
        )
    };
    if status != 200 {
        return Err(ErroHttp::Status(status));
    }

    let mut corpo = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let mut lidos: u32 = 0;
        let ok = unsafe {
            WinHttpReadData(
                pedido.0,
                buf.as_mut_ptr() as *mut c_void,
                buf.len() as u32,
                &mut lidos,
            )
        };
        if ok == 0 {
            return Err(ErroHttp::Rede(ultimo_erro()));
        }
        if lidos == 0 {
            break;
        }
        corpo.extend_from_slice(&buf[..lidos as usize]);
        if corpo.len() > limite {
            return Err(ErroHttp::Grande);
        }
        progresso(corpo.len() as u64);
    }
    Ok(corpo)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assinatura_digital() {
        // unsigned file
        let p =
            std::env::temp_dir().join(format!("vpn-teste-assinatura-{}.exe", std::process::id()));
        std::fs::write(&p, b"MZ nada assinado").unwrap();
        assert_eq!(assinante(&p), None);
        let _ = std::fs::remove_file(&p);

        // the official MSI embedded in the release is signed by OpenVPN Inc.
        let msi = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("openvpn.msi");
        if msi.exists() {
            let nome = assinante(&msi).expect("the OpenVPN MSI should be signed");
            assert!(nome.contains("OpenVPN"), "{nome}");
        }
    }
}
