//! Account backup: every account (usernames, passwords, seeds) and its
//! .ovpn files in one file protected by a password the user chooses, to take
//! the accounts to another computer or Windows account. The usual storage
//! (DPAPI, see `dpapi`) only opens for the same Windows user, so it cannot do
//! that.
//!
//! File: "VPNBKP01" | salt (16) | nonce (12) | Argon2id memory KiB, passes,
//! lanes (u32 LE each) | AES-256-GCM(JSON). The header is authenticated too.

use crate::accounts::Conta;
use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

const MAGICO: &[u8; 8] = b"VPNBKP01";
const TAM_SAL: usize = 16;
const TAM_NONCE: usize = 12;
const TAM_CABECALHO: usize = MAGICO.len() + TAM_SAL + TAM_NONCE + 12;
/// Argon2id: 64 MiB, 3 passes, 1 lane (a fraction of a second; slow for
/// anyone trying passwords by brute force).
const ARGON_MEMORIA_KIB: u32 = 64 * 1024;
const ARGON_PASSADAS: u32 = 3;
const ARGON_FAIXAS: u32 = 1;
pub const SENHA_MINIMA: usize = 8;
/// Largest file taken along with an account (a .ovpn and its keys are a few KB).
const MAIOR_ARQUIVO: u64 = 5 * 1024 * 1024;
/// .ovpn directives whose argument is a file next to it.
const DIRETIVAS_DE_ARQUIVO: [&str; 12] = [
    "ca",
    "cert",
    "key",
    "pkcs12",
    "tls-auth",
    "tls-crypt",
    "tls-crypt-v2",
    "dh",
    "extra-certs",
    "secret",
    "auth-user-pass",
    "askpass",
];

#[derive(Serialize, Deserialize)]
struct Conteudo {
    versao: u32,
    app: String,
    contas: Vec<ContaExportada>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ContaExportada {
    pub conta: Conta,
    /// The .ovpn (first) and the files it references, by path relative to it.
    arquivos: Vec<Arquivo>,
}

#[derive(Serialize, Deserialize, Clone)]
struct Arquivo {
    nome: String,
    dados: Vec<u8>,
}

/// Result of an export: files referenced by a .ovpn that could not go along
/// (absolute path or outside its folder), to warn the user.
pub struct Exportado {
    pub contas: usize,
    pub nao_incluidos: Vec<String>,
}

// ---------------------------------------------------------------- export --

pub fn exportar(contas: &[Conta], senha: &str, destino: &Path) -> Result<Exportado, String> {
    let mut nao_incluidos = Vec::new();
    let mut lista = Vec::new();
    for conta in contas {
        let (arquivos, faltam) = arquivos_da_conta(conta)?;
        nao_incluidos.extend(faltam.into_iter().map(|f| format!("{}: {f}", conta.nome_exibicao())));
        lista.push(ContaExportada {
            conta: conta.clone(),
            arquivos,
        });
    }
    let conteudo = Conteudo {
        versao: 1,
        app: crate::update::VERSAO_ATUAL.to_string(),
        contas: lista,
    };
    let json = serde_json::to_vec(&conteudo).map_err(|e| e.to_string())?;
    let dados = cifrar(&json, senha)?;
    // temporary file + rename: a failure halfway does not leave half a backup
    let temp = destino.with_extension("tmp");
    std::fs::write(&temp, dados)
        .and_then(|_| std::fs::rename(&temp, destino))
        .map_err(|e| {
            let _ = std::fs::remove_file(&temp);
            trf!("Não foi possível gravar o arquivo: {e}", "Could not write the file: {e}")
        })?;
    Ok(Exportado {
        contas: contas.len(),
        nao_incluidos,
    })
}

/// The account's .ovpn plus the files it references by a relative path.
/// Returns also the references that cannot go along.
fn arquivos_da_conta(conta: &Conta) -> Result<(Vec<Arquivo>, Vec<String>), String> {
    let config = Path::new(&conta.config);
    let ler = |p: &Path| -> Result<Vec<u8>, String> {
        let tamanho = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        if tamanho > MAIOR_ARQUIVO {
            return Err(trf!("{} é grande demais.", "{} is too large.", p.display()));
        }
        std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))
    };
    let ovpn = ler(config)?;
    let nome = config
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config.ovpn".into());
    let pasta = config.parent().unwrap_or(Path::new("."));
    let mut arquivos = vec![Arquivo { nome, dados: ovpn.clone() }];
    let mut faltam = Vec::new();
    for referencia in referencias(&String::from_utf8_lossy(&ovpn)) {
        let caminho = Path::new(&referencia);
        if !relativo_seguro(caminho) {
            faltam.push(referencia);
            continue;
        }
        let completo = pasta.join(caminho);
        if !completo.is_file() {
            faltam.push(referencia);
            continue;
        }
        if !arquivos.iter().any(|a| a.nome == referencia) {
            arquivos.push(Arquivo {
                dados: ler(&completo)?,
                nome: referencia,
            });
        }
    }
    Ok((arquivos, faltam))
}

/// File arguments of the .ovpn directives (not inline blocks).
fn referencias(ovpn: &str) -> Vec<String> {
    let mut lista = Vec::new();
    for linha in ovpn.lines() {
        let linha = linha.trim();
        if linha.starts_with('#') || linha.starts_with(';') {
            continue;
        }
        let mut partes = linha.splitn(2, char::is_whitespace);
        let (Some(diretiva), Some(resto)) = (partes.next(), partes.next()) else {
            continue;
        };
        if !DIRETIVAS_DE_ARQUIVO.contains(&diretiva) {
            continue;
        }
        let resto = resto.trim();
        let arquivo = match resto.strip_prefix('"') {
            Some(r) => r.split('"').next().unwrap_or(""),
            None => resto.split_whitespace().next().unwrap_or(""),
        };
        if !arquivo.is_empty() && arquivo != "[inline]" && !lista.iter().any(|a| a == arquivo) {
            lista.push(arquivo.to_string());
        }
    }
    lista
}

/// Relative path that stays inside its folder (no "..", drive or root).
fn relativo_seguro(p: &Path) -> bool {
    !p.as_os_str().is_empty() && p.components().all(|c| matches!(c, Component::Normal(_)))
}

// ---------------------------------------------------------------- import --

/// Opens a backup. Wrong password and damaged file give the same message:
/// the encryption cannot tell them apart.
pub fn abrir(arquivo: &Path, senha: &str) -> Result<Vec<ContaExportada>, String> {
    let dados = std::fs::read(arquivo)
        .map_err(|e| trf!("Não foi possível ler o arquivo: {e}", "Could not read the file: {e}"))?;
    let json = decifrar(&dados, senha)?;
    let conteudo: Conteudo = serde_json::from_slice(&json).map_err(|_| arquivo_invalido())?;
    if conteudo.versao != 1 {
        return Err(tr!(
            "Este backup foi feito por uma versão mais nova do app. Atualize o app e tente de novo.",
            "This backup was made by a newer version of the app. Update the app and try again."
        )
        .into());
    }
    for c in &conteudo.contas {
        let primeiro_ok = c.arquivos.first().is_some_and(|a| nome_de_arquivo_simples(&a.nome));
        if !primeiro_ok || !c.arquivos.iter().all(|a| relativo_seguro(Path::new(&a.nome))) {
            return Err(arquivo_invalido());
        }
    }
    Ok(conteudo.contas)
}

/// Puts the .ovpn files in place and returns the accounts ready to save.
/// The account keeps its original .ovpn path when that file is still there
/// with the same content (same computer); otherwise the files go to
/// %APPDATA%\VPN\configs\<account id>\.
pub fn instalar(contas: &[ContaExportada], base: &Path) -> Result<Vec<Conta>, String> {
    let mut prontas = Vec::new();
    for c in contas {
        let mut conta = c.conta.clone();
        let ovpn = &c.arquivos[0];
        let original = Path::new(&conta.config);
        let igual = std::fs::read(original).is_ok_and(|d| d == ovpn.dados);
        if !igual {
            let pasta = base.join(id_de_pasta(&conta.id));
            for a in &c.arquivos {
                let destino = pasta.join(&a.nome);
                if let Some(pai) = destino.parent() {
                    std::fs::create_dir_all(pai).map_err(|e| e.to_string())?;
                }
                std::fs::write(&destino, &a.dados).map_err(|e| {
                    trf!(
                        "Não foi possível gravar {}: {e}",
                        "Could not write {}: {e}",
                        destino.display()
                    )
                })?;
            }
            conta.config = pasta.join(&ovpn.nome).to_string_lossy().into_owned();
        }
        prontas.push(conta);
    }
    Ok(prontas)
}

fn nome_de_arquivo_simples(nome: &str) -> bool {
    let p = Path::new(nome);
    relativo_seguro(p) && p.components().count() == 1
}

/// Folder name from the account id (only letters, digits, '-' and '_').
fn id_de_pasta(id: &str) -> PathBuf {
    let limpo: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(64)
        .collect();
    PathBuf::from(if limpo.is_empty() { "conta".to_string() } else { limpo })
}

fn arquivo_invalido() -> String {
    tr!(
        "Senha errada, ou o arquivo não é um backup de contas do VPN.",
        "Wrong password, or the file is not a VPN account backup."
    )
    .into()
}

// ---------------------------------------------------------- encryption --

fn chave(senha: &str, sal: &[u8], memoria: u32, passadas: u32, faixas: u32) -> Result<[u8; 32], String> {
    let params = Params::new(memoria, passadas, faixas, Some(32)).map_err(|_| arquivo_invalido())?;
    let mut chave = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(senha.as_bytes(), sal, &mut chave)
        .map_err(|_| arquivo_invalido())?;
    Ok(chave)
}

fn cifrar(dados: &[u8], senha: &str) -> Result<Vec<u8>, String> {
    let mut sal = [0u8; TAM_SAL];
    let mut nonce = [0u8; TAM_NONCE];
    getrandom::getrandom(&mut sal)
        .and_then(|_| getrandom::getrandom(&mut nonce))
        .map_err(|e| e.to_string())?;
    let mut saida = Vec::with_capacity(TAM_CABECALHO + dados.len() + 16);
    saida.extend_from_slice(MAGICO);
    saida.extend_from_slice(&sal);
    saida.extend_from_slice(&nonce);
    for v in [ARGON_MEMORIA_KIB, ARGON_PASSADAS, ARGON_FAIXAS] {
        saida.extend_from_slice(&v.to_le_bytes());
    }
    let mut k = chave(senha, &sal, ARGON_MEMORIA_KIB, ARGON_PASSADAS, ARGON_FAIXAS)?;
    let cifra = Aes256Gcm::new_from_slice(&k).map_err(|e| e.to_string());
    k.fill(0);
    let cifrado = cifra?
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: dados, aad: &saida })
        .map_err(|e| e.to_string())?;
    saida.extend_from_slice(&cifrado);
    Ok(saida)
}

fn decifrar(dados: &[u8], senha: &str) -> Result<Vec<u8>, String> {
    if dados.len() < TAM_CABECALHO + 16 || &dados[..MAGICO.len()] != MAGICO {
        return Err(tr!(
            "Este arquivo não é um backup de contas do VPN.",
            "This file is not a VPN account backup."
        )
        .into());
    }
    let (cabecalho, cifrado) = dados.split_at(TAM_CABECALHO);
    let sal = &cabecalho[8..8 + TAM_SAL];
    let nonce = &cabecalho[8 + TAM_SAL..8 + TAM_SAL + TAM_NONCE];
    let numero = |i: usize| {
        let ini = 8 + TAM_SAL + TAM_NONCE + i * 4;
        u32::from_le_bytes(cabecalho[ini..ini + 4].try_into().unwrap())
    };
    let (memoria, passadas, faixas) = (numero(0), numero(1), numero(2));
    // a crafted file could ask for absurd amounts of memory or time
    if memoria > 1024 * 1024 || passadas > 16 || faixas > 16 {
        return Err(arquivo_invalido());
    }
    let mut k = chave(senha, sal, memoria, passadas, faixas)?;
    let cifra = Aes256Gcm::new_from_slice(&k).map_err(|e| e.to_string());
    k.fill(0);
    cifra?
        .decrypt(Nonce::from_slice(nonce), Payload { msg: cifrado, aad: cabecalho })
        .map_err(|_| arquivo_invalido())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::Autenticacao;

    fn dir_temp(nome: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("vpn-backup-{nome}-{}", crate::accounts::novo_id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn encontra_arquivos_referenciados() {
        let ovpn = "client\nca ca.crt\ncert \"pasta/meu cert.crt\"\n# key comentada.key\n\
                    key [inline]\n<tls-auth>\n-----\n</tls-auth>\ntls-crypt ../fora.key\n\
                    auth-user-pass\nca ca.crt\nremote vpn.exemplo 1194\n";
        assert_eq!(referencias(ovpn), ["ca.crt", "pasta/meu cert.crt", "../fora.key"]);
        assert!(relativo_seguro(Path::new("pasta/meu cert.crt")));
        for ruim in ["../fora.key", r"C:\chaves\a.key", r"\a.key", ""] {
            assert!(!relativo_seguro(Path::new(ruim)), "{ruim}");
        }
    }

    #[test]
    fn exporta_e_importa_com_a_senha_certa() {
        let origem = dir_temp("origem");
        std::fs::write(origem.join("trabalho.ovpn"), "client\nauth-user-pass\nca ca.crt\nkey C:\\fora\\x.key\n").unwrap();
        std::fs::write(origem.join("ca.crt"), "CERTIFICADO").unwrap();
        let conta = Conta {
            id: "c123-teste".into(),
            nome: "Trabalho".into(),
            config: origem.join("trabalho.ovpn").to_string_lossy().into_owned(),
            usuario: "maria".into(),
            autenticacao: Autenticacao::SenhaMaisToken,
            senha: "s3nha".into(),
            seed: "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ".into(),
            ..Default::default()
        };
        let backup = origem.join("contas.vpnbackup");
        let r = exportar(std::slice::from_ref(&conta), "senha-forte-1", &backup).unwrap();
        assert_eq!(r.contas, 1);
        assert_eq!(r.nao_incluidos, ["Trabalho: C:\\fora\\x.key"]);
        let bruto = std::fs::read(&backup).unwrap();
        assert!(bruto.starts_with(MAGICO));
        // nothing readable inside
        let texto = String::from_utf8_lossy(&bruto);
        for segredo in ["s3nha", "GEZDGNBV", "CERTIFICADO", "maria"] {
            assert!(!texto.contains(segredo), "{segredo}");
        }

        assert_eq!(abrir(&backup, "senha-errada").err().unwrap(), arquivo_invalido());
        let contas = abrir(&backup, "senha-forte-1").unwrap();
        assert_eq!(contas.len(), 1);
        assert_eq!(contas[0].conta, conta);

        // same computer: keeps the original .ovpn
        let base = dir_temp("destino");
        let prontas = instalar(&contas, &base).unwrap();
        assert_eq!(prontas[0].config, conta.config);

        // another computer (the .ovpn is gone): files recreated under base
        std::fs::remove_dir_all(&origem).unwrap();
        let prontas = instalar(&contas, &base).unwrap();
        let novo = Path::new(&prontas[0].config);
        assert_eq!(novo, base.join("c123-teste").join("trabalho.ovpn"));
        assert_eq!(std::fs::read_to_string(base.join("c123-teste").join("ca.crt")).unwrap(), "CERTIFICADO");
        assert_eq!(prontas[0].senha, "s3nha");
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn recusa_arquivo_alterado_ou_estranho() {
        let dir = dir_temp("alterado");
        let p = dir.join("b.vpnbackup");
        std::fs::write(&p, cifrar(b"{\"versao\":1,\"app\":\"x\",\"contas\":[]}", "senha-forte-1").unwrap()).unwrap();
        assert!(abrir(&p, "senha-forte-1").unwrap().is_empty());
        // one changed byte (in the header or in the data) breaks the authentication
        for i in [MAGICO.len() + 1, TAM_CABECALHO + 3] {
            let mut d = std::fs::read(&p).unwrap();
            d[i] ^= 1;
            let q = dir.join("alterado.vpnbackup");
            std::fs::write(&q, d).unwrap();
            assert!(abrir(&q, "senha-forte-1").is_err(), "byte {i}");
        }
        std::fs::write(&p, b"qualquer coisa").unwrap();
        assert!(abrir(&p, "senha-forte-1").err().unwrap().contains("não é um backup"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn nomes_perigosos_no_backup_sao_recusados() {
        let dir = dir_temp("perigoso");
        let conteudo = Conteudo {
            versao: 1,
            app: "x".into(),
            contas: vec![ContaExportada {
                conta: Conta::nova(),
                arquivos: vec![Arquivo {
                    nome: "..\\..\\Windows\\x.ovpn".into(),
                    dados: vec![],
                }],
            }],
        };
        let p = dir.join("b.vpnbackup");
        std::fs::write(&p, cifrar(&serde_json::to_vec(&conteudo).unwrap(), "senha-forte-1").unwrap()).unwrap();
        assert_eq!(abrir(&p, "senha-forte-1").err().unwrap(), arquivo_invalido());
        assert_eq!(id_de_pasta("..\\x/y"), PathBuf::from("xy"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
