//! Contas de VPN: modelo, autenticacao e verificacoes do arquivo .ovpn.

use crate::totp;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// Como a senha enviada ao OpenVPN e formada.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Autenticacao {
    /// A senha e o token TOTP gerado na hora (Google Authenticator).
    #[default]
    Token,
    /// Senha fixa, sem token.
    Senha,
    /// Senha fixa seguida do token de 6 digitos ("senha123456").
    SenhaMaisToken,
}

impl Autenticacao {
    pub const TODAS: [Autenticacao; 3] = [
        Autenticacao::Token,
        Autenticacao::Senha,
        Autenticacao::SenhaMaisToken,
    ];

    pub fn rotulo(self) -> &'static str {
        match self {
            Autenticacao::Token => "Token (Google Authenticator)",
            Autenticacao::Senha => "Senha fixa",
            Autenticacao::SenhaMaisToken => "Senha + token",
        }
    }

    pub fn usa_token(self) -> bool {
        matches!(self, Autenticacao::Token | Autenticacao::SenhaMaisToken)
    }

    pub fn usa_senha(self) -> bool {
        matches!(self, Autenticacao::Senha | Autenticacao::SenhaMaisToken)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Conta {
    pub id: String,
    pub nome: String,
    /// Caminho completo do arquivo .ovpn.
    pub config: String,
    pub usuario: String,
    #[serde(default)]
    pub autenticacao: Autenticacao,
    /// Seed base32 (normalizada) quando a autenticacao usa token.
    #[serde(default)]
    pub seed: String,
    /// Senha fixa quando a autenticacao usa senha.
    #[serde(default)]
    pub senha: String,
}

static CONTADOR_ID: AtomicU64 = AtomicU64::new(0);

/// Identificador unico e estavel de conta (nunca reaproveitado).
pub fn novo_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let seq = CONTADOR_ID.fetch_add(1, Ordering::SeqCst);
    format!("c{nanos:x}{seq:x}")
}

impl Conta {
    pub fn nova() -> Self {
        Conta {
            id: novo_id(),
            ..Default::default()
        }
    }

    pub fn nome_exibicao(&self) -> &str {
        let nome = self.nome.trim();
        if nome.is_empty() {
            "Sem nome"
        } else {
            nome
        }
    }

    /// Nome do arquivo .ovpn (sem o caminho), para exibir.
    pub fn arquivo(&self) -> String {
        Path::new(&self.config)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Confere se a conta tem tudo o que precisa para conectar.
    /// Retorna a mensagem para o usuario quando falta algo.
    pub fn validar(&self) -> Result<(), String> {
        if self.nome.trim().is_empty() {
            return Err("Informe um nome para a conta.".into());
        }
        if self.config.trim().is_empty() || !Path::new(&self.config).exists() {
            return Err("Escolha um arquivo .ovpn válido.".into());
        }
        if self.usuario.trim().is_empty() {
            return Err("Informe o usuário.".into());
        }
        if self.autenticacao.usa_token() && totp::normalize_seed(&self.seed).is_none() {
            return Err(
                "Seed inválida (não é base32). Copie a chave do cadastro do \
                 Google Authenticator ou use Importar QR Code."
                    .into(),
            );
        }
        if self.autenticacao.usa_senha() && self.senha.is_empty() {
            return Err("Informe a senha.".into());
        }
        Ok(())
    }

    /// Senha a enviar ao OpenVPN. `token` e o codigo TOTP da janela atual
    /// (obrigatorio quando a autenticacao usa token).
    pub fn compor_senha(&self, token: Option<&str>) -> Option<String> {
        match self.autenticacao {
            Autenticacao::Token => token.map(str::to_string),
            Autenticacao::Senha => Some(self.senha.clone()),
            Autenticacao::SenhaMaisToken => token.map(|t| format!("{}{t}", self.senha)),
        }
    }
}

/// True quando o .ovpn manda TODO o trafego pela VPN (`redirect-gateway`).
/// Duas conexoes assim ao mesmo tempo disputam a rota padrao e a ultima a
/// conectar vence. So enxerga a diretiva do arquivo: o servidor tambem pode
/// enviar essa rota, e isso so se descobre depois de conectar.
pub fn redireciona_tudo(config: &Path) -> bool {
    let Ok(texto) = std::fs::read_to_string(config) else {
        return false;
    };
    texto.lines().any(|linha| {
        let linha = linha.trim();
        !linha.starts_with('#')
            && !linha.starts_with(';')
            && linha.split_whitespace().next() == Some("redirect-gateway")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEED: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";

    fn conta_valida(dir: &Path) -> Conta {
        let cfg = dir.join("teste.ovpn");
        std::fs::write(&cfg, "client\n").unwrap();
        Conta {
            id: novo_id(),
            nome: "Teste".into(),
            config: cfg.to_string_lossy().into_owned(),
            usuario: "usuario.teste".into(),
            autenticacao: Autenticacao::Token,
            seed: SEED.into(),
            senha: String::new(),
        }
    }

    fn dir_temp(nome: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("vpn-teste-{nome}-{}", novo_id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn senha_composta_pelos_tres_modos() {
        let mut c = Conta {
            senha: "segredo".into(),
            ..Default::default()
        };
        c.autenticacao = Autenticacao::Token;
        assert_eq!(c.compor_senha(Some("123456")).as_deref(), Some("123456"));
        assert_eq!(c.compor_senha(None), None);

        c.autenticacao = Autenticacao::Senha;
        assert_eq!(c.compor_senha(None).as_deref(), Some("segredo"));

        c.autenticacao = Autenticacao::SenhaMaisToken;
        assert_eq!(
            c.compor_senha(Some("123456")).as_deref(),
            Some("segredo123456")
        );
    }

    #[test]
    fn validacao_aponta_o_que_falta() {
        let dir = dir_temp("validar");
        let ok = conta_valida(&dir);
        assert!(ok.validar().is_ok());

        let mut c = ok.clone();
        c.nome = "  ".into();
        assert!(c.validar().unwrap_err().contains("nome"));

        let mut c = ok.clone();
        c.config = dir.join("nao-existe.ovpn").to_string_lossy().into_owned();
        assert!(c.validar().unwrap_err().contains(".ovpn"));

        let mut c = ok.clone();
        c.seed = "123!".into();
        assert!(c.validar().unwrap_err().contains("Seed"));

        // senha fixa nao exige seed, mas exige senha
        let mut c = ok.clone();
        c.autenticacao = Autenticacao::Senha;
        c.seed.clear();
        assert!(c.validar().unwrap_err().contains("senha"));
        c.senha = "x".into();
        assert!(c.validar().is_ok());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn detecta_redirect_gateway_ignorando_comentarios() {
        let dir = dir_temp("rgw");
        let a = dir.join("a.ovpn");
        std::fs::write(&a, "client\nredirect-gateway def1\n").unwrap();
        assert!(redireciona_tudo(&a));

        let b = dir.join("b.ovpn");
        std::fs::write(&b, "client\n# redirect-gateway def1\n;redirect-gateway\nroute 10.0.0.0 255.0.0.0\n").unwrap();
        assert!(!redireciona_tudo(&b));

        assert!(!redireciona_tudo(&dir.join("nao-existe.ovpn")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ids_sao_unicos() {
        let ids: std::collections::HashSet<String> = (0..500).map(|_| novo_id()).collect();
        assert_eq!(ids.len(), 500);
    }
}
