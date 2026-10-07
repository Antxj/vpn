//! VPN accounts: model, authentication and checks on the .ovpn file.

use crate::totp;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// How the password sent to OpenVPN is built.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Autenticacao {
    /// The password is the TOTP token generated on the spot (Google Authenticator).
    #[default]
    Token,
    /// Fixed password, no token.
    Senha,
    /// Fixed password followed by the 6-digit token ("password123456").
    SenhaMaisToken,
    /// No username/password: the VPN authenticates with the certificate in
    /// the .ovpn only (the file has no `auth-user-pass`).
    SoCertificado,
}

impl Autenticacao {
    pub const TODAS: [Autenticacao; 4] = [
        Autenticacao::Token,
        Autenticacao::Senha,
        Autenticacao::SenhaMaisToken,
        Autenticacao::SoCertificado,
    ];

    pub fn rotulo(self) -> &'static str {
        match self {
            Autenticacao::Token => tr!("Token (Google Authenticator)", "Token (Google Authenticator)"),
            Autenticacao::Senha => tr!("Senha fixa", "Fixed password"),
            Autenticacao::SenhaMaisToken => tr!("Senha + token", "Password + token"),
            Autenticacao::SoCertificado => tr!("Só certificado", "Certificate only"),
        }
    }

    /// Asks for a username (every mode except certificate only).
    pub fn usa_usuario(self) -> bool {
        self != Autenticacao::SoCertificado
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
    /// Full path of the .ovpn file.
    pub config: String,
    pub usuario: String,
    #[serde(default)]
    pub autenticacao: Autenticacao,
    /// Base32 seed (normalized) when the authentication uses a token.
    #[serde(default)]
    pub seed: String,
    /// Fixed password when the authentication uses a password.
    #[serde(default)]
    pub senha: String,
    /// Tunnel type observed on the last connection: Some(true) = all internet
    /// traffic goes through the VPN; Some(false) = only the VPN network. None =
    /// not connected yet (or the .ovpn file changed since then).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tunel_completo: Option<bool>,
}

static CONTADOR_ID: AtomicU64 = AtomicU64::new(0);

/// Unique, stable account identifier (never reused).
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
            tr!("Sem nome", "Unnamed")
        } else {
            nome
        }
    }

    /// Does all internet traffic go through this VPN? Uses what was observed on
    /// the last connection; without that, what the .ovpn file says (the server
    /// may still push the default route, which is only known after connecting).
    pub fn tunel_completo(&self) -> Option<bool> {
        self.tunel_completo
            .or_else(|| redireciona_tudo(Path::new(&self.config)).then_some(true))
    }

    /// Name of the .ovpn file (without the path), for display.
    pub fn arquivo(&self) -> String {
        Path::new(&self.config)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Checks whether the account has everything it needs to connect.
    /// Returns the message for the user when something is missing.
    pub fn validar(&self) -> Result<(), String> {
        if self.nome.trim().is_empty() {
            return Err(tr!("Informe um nome para a conta.", "Enter a name for the account.").into());
        }
        if self.config.trim().is_empty() || !Path::new(&self.config).exists() {
            return Err(tr!("Escolha um arquivo .ovpn válido.", "Choose a valid .ovpn file.").into());
        }
        // the .ovpn decides whether OpenVPN asks for a username and password
        let pede = pede_usuario_e_senha(Path::new(&self.config));
        if self.autenticacao == Autenticacao::SoCertificado && pede {
            return Err(tr!(
                "Este arquivo .ovpn pede usuário e senha (auth-user-pass). \
                 Escolha outra forma de autenticação.",
                "This .ovpn file asks for a username and password (auth-user-pass). \
                 Choose another authentication method."
            )
            .into());
        }
        if self.autenticacao != Autenticacao::SoCertificado && !pede {
            return Err(tr!(
                "Este arquivo .ovpn não pede usuário e senha: a conexão usa só o \
                 certificado. Escolha \"Só certificado\".",
                "This .ovpn file does not ask for a username and password: the \
                 connection uses the certificate only. Choose \"Certificate only\"."
            )
            .into());
        }
        if self.autenticacao.usa_usuario() && self.usuario.trim().is_empty() {
            return Err(tr!("Informe o usuário.", "Enter the username.").into());
        }
        if self.autenticacao.usa_token() && totp::normalize_seed(&self.seed).is_none() {
            return Err(tr!(
                "Seed inválida (não é base32). Copie a chave do cadastro do \
                 Google Authenticator ou use Importar QR Code.",
                "Invalid seed (not base32). Copy the key from the Google \
                 Authenticator enrollment or use Import QR code."
            )
            .into());
        }
        if self.autenticacao.usa_senha() && self.senha.is_empty() {
            return Err(tr!("Informe a senha.", "Enter the password.").into());
        }
        Ok(())
    }

    /// Password to send to OpenVPN. `token` is the TOTP code of the current
    /// window (required when the authentication uses a token).
    pub fn compor_senha(&self, token: Option<&str>) -> Option<String> {
        match self.autenticacao {
            Autenticacao::Token => token.map(str::to_string),
            Autenticacao::Senha => Some(self.senha.clone()),
            Autenticacao::SenhaMaisToken => token.map(|t| format!("{}{t}", self.senha)),
            Autenticacao::SoCertificado => None,
        }
    }
}

/// True when the .ovpn makes OpenVPN ask for a username and password
/// (`auth-user-pass`). Without it, the connection uses the certificate only.
pub fn pede_usuario_e_senha(config: &Path) -> bool {
    tem_diretiva(config, "auth-user-pass")
}

/// The .ovpn has the directive (ignoring comments).
fn tem_diretiva(config: &Path, diretiva: &str) -> bool {
    let Ok(texto) = std::fs::read_to_string(config) else {
        return false;
    };
    texto.lines().any(|linha| {
        let linha = linha.trim();
        !linha.starts_with('#')
            && !linha.starts_with(';')
            && linha.split_whitespace().next() == Some(diretiva)
    })
}

/// True when the .ovpn sends ALL traffic through the VPN (`redirect-gateway`).
/// Two such connections at the same time fight over the default route and
/// the last one to connect wins. Only sees the file directive: the server
/// can also push that route, which is only known after connecting.
pub fn redireciona_tudo(config: &Path) -> bool {
    tem_diretiva(config, "redirect-gateway")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEED: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";

    fn conta_valida(dir: &Path) -> Conta {
        let cfg = dir.join("teste.ovpn");
        std::fs::write(&cfg, "client\nauth-user-pass\n").unwrap();
        Conta {
            id: novo_id(),
            nome: "Teste".into(),
            config: cfg.to_string_lossy().into_owned(),
            usuario: "usuario.teste".into(),
            autenticacao: Autenticacao::Token,
            seed: SEED.into(),
            senha: String::new(),
            tunel_completo: None,
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

        // certificate only: never answers a username/password prompt
        c.autenticacao = Autenticacao::SoCertificado;
        assert_eq!(c.compor_senha(Some("123456")), None);
    }

    #[test]
    fn so_certificado_segue_o_arquivo_ovpn() {
        let dir = dir_temp("certificado");
        let so_cert = dir.join("so-cert.ovpn");
        std::fs::write(&so_cert, "client\n# auth-user-pass (commented out)\nremote x 1194\n").unwrap();
        let com_senha = dir.join("com-senha.ovpn");
        std::fs::write(&com_senha, "client\n  auth-user-pass\nremote x 1194\n").unwrap();
        assert!(!pede_usuario_e_senha(&so_cert));
        assert!(pede_usuario_e_senha(&com_senha));

        // certificate only: no username, seed or password needed
        let c = Conta {
            id: novo_id(),
            nome: "Cert".into(),
            config: so_cert.to_string_lossy().into_owned(),
            autenticacao: Autenticacao::SoCertificado,
            ..Default::default()
        };
        assert!(c.validar().is_ok(), "{:?}", c.validar());
        assert!(!c.autenticacao.usa_usuario());

        // the mode has to match the file, both ways
        let mut errado = c.clone();
        errado.config = com_senha.to_string_lossy().into_owned();
        assert!(errado.validar().unwrap_err().contains("auth-user-pass"));
        let mut errado = c.clone();
        errado.autenticacao = Autenticacao::Senha;
        errado.usuario = "u".into();
        errado.senha = "p".into();
        assert!(errado.validar().unwrap_err().contains("Só certificado"));

        let _ = std::fs::remove_dir_all(dir);
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

        // a fixed password does not require a seed, but does require the password
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
