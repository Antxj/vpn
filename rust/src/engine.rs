//! Engine: holds the accounts and the active connections. It is global and
//! thread-safe because it is used both by the UI and by the tray menu (which
//! works with the window hidden, when the egui loop does not run).

use crate::accounts::Conta;
use crate::dpapi::{self, Settings};
use crate::state::{self, Situacao};
use crate::vpn;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Why a connection cannot be started.
#[derive(Debug, PartialEq)]
pub enum ErroConexao {
    OpenVpnAusente,
    ContaInexistente,
    /// Message ready for the user (incomplete data, conflict etc.).
    Invalida(String),
}

impl ErroConexao {
    pub fn mensagem(&self) -> String {
        match self {
            ErroConexao::OpenVpnAusente => tr!(
                "O OpenVPN Community não está instalado neste computador.",
                "OpenVPN Community is not installed on this computer."
            )
            .into(),
            ErroConexao::ContaInexistente => tr!("Conta não encontrada.", "Account not found.").into(),
            ErroConexao::Invalida(m) => m.clone(),
        }
    }
}

pub struct Motor {
    settings: Mutex<Settings>,
    conexoes: Mutex<HashMap<String, vpn::Controller>>,
}

static MOTOR: OnceLock<Motor> = OnceLock::new();

/// The process engine (loads the saved accounts on the first call).
pub fn get() -> &'static Motor {
    MOTOR.get_or_init(|| Motor {
        settings: Mutex::new(dpapi::load_settings()),
        conexoes: Mutex::new(HashMap::new()),
    })
}

impl Motor {
    // ---------------------------------------------------------- accounts --

    pub fn contas(&self) -> Vec<Conta> {
        self.settings.lock().unwrap().contas.clone()
    }

    pub fn conta(&self, id: &str) -> Option<Conta> {
        self.settings
            .lock()
            .unwrap()
            .contas
            .iter()
            .find(|c| c.id == id)
            .cloned()
    }

    /// (id, name) of each account, in list order.
    pub fn nomes(&self) -> Vec<(String, String)> {
        self.settings
            .lock()
            .unwrap()
            .contas
            .iter()
            .map(|c| (c.id.clone(), c.nome_exibicao().to_string()))
            .collect()
    }

    /// Adds or updates an account (by id) and saves it to disk.
    pub fn salvar_conta(&self, mut conta: Conta) {
        let mut s = self.settings.lock().unwrap();
        match s.contas.iter_mut().find(|c| c.id == conta.id) {
            Some(existente) => {
                // another .ovpn: the observed tunnel type no longer applies
                if existente.config != conta.config {
                    conta.tunel_completo = None;
                }
                *existente = conta;
            }
            None => s.contas.push(conta),
        }
        dpapi::save_settings(&s);
    }

    /// Stores the tunnel type observed on connect (saves only if it changed).
    pub fn registrar_tunel(&self, id: &str, completo: bool) -> bool {
        let mut s = self.settings.lock().unwrap();
        let Some(c) = s.contas.iter_mut().find(|c| c.id == id) else {
            return false;
        };
        if c.tunel_completo == Some(completo) {
            return false;
        }
        c.tunel_completo = Some(completo);
        dpapi::save_settings(&s);
        true
    }

    /// Chosen language (None = automatic from Windows).
    pub fn idioma(&self) -> Option<crate::i18n::Idioma> {
        let s = self.settings.lock().unwrap();
        s.idioma.as_deref().and_then(crate::i18n::Idioma::do_codigo)
    }

    pub fn salvar_idioma(&self, idioma: Option<crate::i18n::Idioma>) {
        let mut s = self.settings.lock().unwrap();
        s.idioma = idioma.map(|i| i.codigo().to_string());
        dpapi::save_settings(&s);
        drop(s);
        crate::i18n::aplicar(idioma);
    }

    pub fn remover_conta(&self, id: &str) {
        self.desconectar(id);
        let mut s = self.settings.lock().unwrap();
        s.contas.retain(|c| c.id != id);
        dpapi::save_settings(&s);
        drop(s);
        state::remover(id);
    }

    pub fn tema_escuro(&self) -> bool {
        self.settings.lock().unwrap().theme.as_deref() != Some("light")
    }

    pub fn salvar_tema(&self, escuro: bool) {
        let mut s = self.settings.lock().unwrap();
        s.theme = Some(if escuro { "dark" } else { "light" }.into());
        dpapi::save_settings(&s);
    }

    /// Check for new versions automatically (default: yes).
    pub fn verifica_atualizacoes(&self) -> bool {
        self.settings.lock().unwrap().atualizacoes != Some(false)
    }

    pub fn salvar_verifica_atualizacoes(&self, ligado: bool) {
        let mut s = self.settings.lock().unwrap();
        s.atualizacoes = if ligado { None } else { Some(false) };
        dpapi::save_settings(&s);
    }

    // ------------------------------------------------------- connections --

    /// Connection in progress (connecting, connected or shutting down).
    pub fn ativa(&self, id: &str) -> bool {
        self.conexoes
            .lock()
            .unwrap()
            .get(id)
            .is_some_and(|c| !c.is_finished())
    }

    pub fn alguma_ativa(&self) -> bool {
        self.conexoes
            .lock()
            .unwrap()
            .values()
            .any(|c| !c.is_finished())
    }

    /// Other active accounts that also send all traffic through the VPN.
    pub fn conflitos_de_rota(&self, id: &str) -> Vec<String> {
        let Some(conta) = self.conta(id) else {
            return Vec::new();
        };
        if conta.tunel_completo() != Some(true) {
            return Vec::new();
        }
        self.contas()
            .into_iter()
            .filter(|c| c.id != id && self.ativa(&c.id))
            .filter(|c| c.tunel_completo() == Some(true))
            .map(|c| c.nome_exibicao().to_string())
            .collect()
    }

    /// Validates the account and starts the connection in the background.
    pub fn conectar(&self, id: &str, openvpn: Option<PathBuf>) -> Result<(), ErroConexao> {
        if self.ativa(id) {
            return Ok(());
        }
        let openvpn = openvpn.ok_or(ErroConexao::OpenVpnAusente)?;
        let conta = self.conta(id).ok_or(ErroConexao::ContaInexistente)?;
        conta.validar().map_err(ErroConexao::Invalida)?;

        // the same .ovpn twice at the same time would fight over the same routes
        let mesma_config = self.contas().into_iter().find(|c| {
            c.id != id
                && self.ativa(&c.id)
                && Path::new(&c.config) == Path::new(&conta.config)
        });
        if let Some(outra) = mesma_config {
            return Err(ErroConexao::Invalida(trf!(
                "A conta \"{}\" já está conectada com o mesmo arquivo .ovpn.",
                "The account \"{}\" is already connected with the same .ovpn file.",
                outra.nome_exibicao()
            )));
        }

        state::log(conta.nome_exibicao(), tr!("Conectando...", "Connecting..."));
        let ctrl = vpn::start(conta, openvpn);
        self.conexoes.lock().unwrap().insert(id.to_string(), ctrl);
        Ok(())
    }

    pub fn desconectar(&self, id: &str) {
        if let Some(c) = self.conexoes.lock().unwrap().get(id) {
            if !c.is_finished() {
                c.request_stop();
                state::definir(id, Situacao::Desconectando, None);
            }
        }
    }

    pub fn desconectar_todas(&self) {
        let ids: Vec<String> = self.conexoes.lock().unwrap().keys().cloned().collect();
        for id in ids {
            self.desconectar(&id);
        }
    }

    /// Disconnects everything (up to 15 s) and exits the process - works even
    /// with the egui loop stopped. Does not return.
    pub fn encerrar(&self) -> ! {
        self.desconectar_todas();
        let limite = Instant::now() + Duration::from_secs(15);
        while Instant::now() < limite && self.alguma_ativa() {
            std::thread::sleep(Duration::from_millis(200));
        }
        std::process::exit(0)
    }
}
