//! "Copy log": the connection log as text to paste in a support message,
//! with personal data and secrets hidden.

use crate::accounts::Conta;
use crate::state::Entrada;
use std::path::Path;

/// Lines taken from the end of OpenVPN's own log (one account's tab).
const LINHAS_OPENVPN: usize = 60;

/// Builds the text: a header, the app log lines (of one account, or all)
/// and, for one account, the end of OpenVPN's own log.
pub fn texto_do_log(entradas: &[Entrada], conta: Option<&Conta>, contas: &[Conta]) -> String {
    let mut texto = format!(
        "VPN {} - {}\n",
        crate::update::VERSAO_ATUAL,
        crate::state::data_e_hora_local()
    );
    for e in entradas {
        if e.origem.is_empty() || conta.is_some() {
            texto.push_str(&format!("{} {}\n", e.hora, e.msg));
        } else {
            texto.push_str(&format!("{} [{}] {}\n", e.hora, e.origem, e.msg));
        }
    }
    if let Some(c) = conta {
        if let Some(fim) = fim_do_arquivo(&crate::vpn::log_path(&c.id), LINHAS_OPENVPN) {
            texto.push_str(&format!(
                "\n--- {} ---\n",
                trf!(
                    "OpenVPN: últimas {LINHAS_OPENVPN} linhas",
                    "OpenVPN: last {LINHAS_OPENVPN} lines"
                )
            ));
            texto.push_str(&fim);
        }
    }
    esconder(&texto, &segredos(contas))
}

fn fim_do_arquivo(caminho: &Path, linhas: usize) -> Option<String> {
    let bruto = std::fs::read(caminho).ok()?;
    let texto = String::from_utf8_lossy(&bruto);
    let todas: Vec<&str> = texto.lines().collect();
    let inicio = todas.len().saturating_sub(linhas);
    Some(todas[inicio..].iter().map(|l| format!("{l}\n")).collect())
}

/// What must not leave the computer: (text, replacement). Account secrets,
/// usernames, the Windows user and the computer name.
fn segredos(contas: &[Conta]) -> Vec<(String, &'static str)> {
    let mut lista: Vec<(String, &'static str)> = Vec::new();
    for c in contas {
        lista.push((c.senha.clone(), "<senha>"));
        lista.push((c.seed.clone(), "<seed>"));
        lista.push((c.usuario.clone(), "<usuario>"));
    }
    if let Ok(perfil) = std::env::var("USERPROFILE") {
        lista.push((perfil, "%USERPROFILE%"));
    }
    for (var, marca) in [("USERNAME", "<usuario-windows>"), ("COMPUTERNAME", "<computador>")] {
        if let Ok(v) = std::env::var(var) {
            lista.push((v, marca));
        }
    }
    // very short values would hide pieces of ordinary words
    lista.retain(|(v, _)| v.trim().chars().count() >= 3);
    // longest first: a path containing the username goes as a whole
    lista.sort_by_key(|(v, _)| std::cmp::Reverse(v.len()));
    lista
}

/// Replaces every occurrence (ignoring ASCII case) of each value.
fn esconder(texto: &str, segredos: &[(String, &'static str)]) -> String {
    let mut saida = texto.to_string();
    for (valor, marca) in segredos {
        saida = trocar_sem_caixa(&saida, valor.trim(), marca);
    }
    saida
}

fn trocar_sem_caixa(texto: &str, de: &str, para: &str) -> String {
    let (t, d) = (texto.as_bytes(), de.as_bytes());
    if d.is_empty() || d.len() > t.len() {
        return texto.to_string();
    }
    let mut saida = String::with_capacity(texto.len());
    let mut i = 0;
    let mut copiado = 0;
    while i + d.len() <= t.len() {
        // compares bytes: non-ASCII characters must match exactly, so a
        // match always starts and ends on character boundaries
        if t[i..i + d.len()].eq_ignore_ascii_case(d) && texto.is_char_boundary(i) {
            saida.push_str(&texto[copiado..i]);
            saida.push_str(para);
            i += d.len();
            copiado = i;
        } else {
            i += 1;
        }
    }
    saida.push_str(&texto[copiado..]);
    saida
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entrada(origem: &str, msg: &str) -> Entrada {
        Entrada {
            hora: "10:00:00".into(),
            origem: origem.into(),
            msg: msg.into(),
        }
    }

    #[test]
    fn esconde_dados_pessoais_e_segredos() {
        let conta = Conta {
            id: "teste-suporte".into(),
            nome: "Trabalho".into(),
            usuario: "maria.silva".into(),
            senha: "Segredo#2026".into(),
            seed: "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ".into(),
            ..Default::default()
        };
        let entradas = [
            entrada("Trabalho", "AUTH: user Maria.Silva rejected"),
            entrada("Trabalho", "password Segredo#2026 seed GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ"),
            entrada("", "app message"),
        ];
        let todas = texto_do_log(&entradas, None, std::slice::from_ref(&conta));
        assert!(todas.starts_with("VPN "), "{todas}");
        assert!(todas.contains("10:00:00 [Trabalho] AUTH: user <usuario> rejected"), "{todas}");
        assert!(todas.contains("password <senha> seed <seed>"), "{todas}");
        assert!(todas.contains("10:00:00 app message"), "{todas}");
        for segredo in ["maria", "Segredo#2026", "GEZDGNBV"] {
            assert!(!todas.to_lowercase().contains(&segredo.to_lowercase()), "{segredo}: {todas}");
        }
        // one account's tab: no name in front of each line
        let uma = texto_do_log(&entradas[..1], Some(&conta), std::slice::from_ref(&conta));
        assert!(uma.contains("10:00:00 AUTH: user <usuario> rejected"), "{uma}");
    }

    #[test]
    fn troca_respeita_acentos_e_valores_curtos() {
        let s = segredos(&[Conta {
            usuario: "jo".into(), // too short: would hide pieces of words
            senha: "ação".into(),
            ..Default::default()
        }]);
        assert!(!s.iter().any(|(v, _)| v == "jo"));
        assert_eq!(esconder("a AÇÃO e a ação", &s), "a AÇÃO e a <senha>");
        assert_eq!(trocar_sem_caixa("abcABCabc", "abc", "x"), "xxx");
        assert_eq!(trocar_sem_caixa("", "abc", "x"), "");
    }
}
