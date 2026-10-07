//! Shared connection state, per account.
//!
//! The connection threads write here DIRECTLY (not through a UI channel):
//! with the window hidden the egui loop does not run, and the tray still
//! has to reflect drops and disconnections. The UI and the tray timer
//! only read.

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

/// Lines kept in the UI log.
const MAX_LOG: usize = 400;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Situacao {
    Desconectado,
    Conectando,
    Conectado,
    Reconectando,
    Desconectando,
}

impl Situacao {
    /// In transition (the icon turns amber).
    pub fn em_transicao(self) -> bool {
        matches!(
            self,
            Situacao::Conectando | Situacao::Reconectando | Situacao::Desconectando
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Traffic {
    pub down_rate: f64,
    pub up_rate: f64,
    pub down_total: u64,
    pub up_total: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EstadoConta {
    pub situacao: Situacao,
    pub ip: Option<String>,
    pub trafego: Option<Traffic>,
}

impl Default for EstadoConta {
    fn default() -> Self {
        EstadoConta {
            situacao: Situacao::Desconectado,
            ip: None,
            trafego: None,
        }
    }
}

impl EstadoConta {
    pub fn texto(&self) -> String {
        match self.situacao {
            Situacao::Desconectado => tr!("Desconectado", "Disconnected").into(),
            Situacao::Conectando => tr!("Conectando...", "Connecting...").into(),
            Situacao::Reconectando => tr!("Reconectando...", "Reconnecting...").into(),
            Situacao::Desconectando => tr!("Desconectando...", "Disconnecting...").into(),
            Situacao::Conectado => match &self.ip {
                Some(ip) => trf!("Conectado  -  IP {ip}", "Connected  -  IP {ip}"),
                None => tr!("Conectado", "Connected").into(),
            },
        }
    }
}

#[derive(Default)]
struct Global {
    contas: HashMap<String, EstadoConta>,
    log: VecDeque<Entrada>,
}

/// One log line: time, account name ("" = the app itself) and message.
#[derive(Clone, Debug, PartialEq)]
pub struct Entrada {
    pub hora: String,
    pub origem: String,
    pub msg: String,
}

#[cfg(test)]
impl Entrada {
    /// "HH:MM:SS [account] message" (or without the account, for the app).
    pub fn texto(&self) -> String {
        if self.origem.is_empty() {
            format!("{} {}", self.hora, self.msg)
        } else {
            format!("{} [{}] {}", self.hora, self.origem, self.msg)
        }
    }
}

type Aviso = Box<dyn Fn() + Send + Sync>;

static GLOBAL: Mutex<Option<Global>> = Mutex::new(None);
static REPINTAR: OnceLock<Aviso> = OnceLock::new();

fn com<R>(f: impl FnOnce(&mut Global) -> R) -> R {
    let mut guard = GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(Global::default))
}

/// Registers who must be notified on every change (the UI asks for a repaint).
pub fn ao_mudar(f: impl Fn() + Send + Sync + 'static) {
    let _ = REPINTAR.set(Box::new(f));
}

fn avisar() {
    if let Some(f) = REPINTAR.get() {
        f();
    }
}

/// Maps the OpenVPN state (>STATE:) to the account status.
pub fn situacao_do_openvpn(estado: &str) -> Situacao {
    match estado {
        "CONNECTED" => Situacao::Conectado,
        "RECONNECTING" => Situacao::Reconectando,
        "EXITING" => Situacao::Desconectando,
        _ => Situacao::Conectando,
    }
}

pub fn definir(conta: &str, situacao: Situacao, ip: Option<String>) {
    com(|g| {
        let e = g.contas.entry(conta.to_string()).or_default();
        e.situacao = situacao;
        if situacao == Situacao::Conectado {
            if ip.is_some() {
                e.ip = ip;
            }
        } else {
            e.ip = None;
            e.trafego = None;
        }
    });
    avisar();
}

pub fn trafego(conta: &str, t: Option<Traffic>) {
    com(|g| {
        if let Some(e) = g.contas.get_mut(conta) {
            if e.situacao == Situacao::Conectado || t.is_none() {
                e.trafego = t;
            }
        }
    });
    avisar();
}

pub fn obter(conta: &str) -> EstadoConta {
    com(|g| g.contas.get(conta).cloned().unwrap_or_default())
}

pub fn remover(conta: &str) {
    com(|g| {
        g.contas.remove(conta);
    });
    avisar();
}

/// Appends a line to the log, prefixed with the time and the account name.
pub fn log(origem: &str, msg: impl AsRef<str>) {
    let linha = Entrada {
        hora: hora_local(),
        origem: origem.to_string(),
        msg: msg.as_ref().to_string(),
    };
    com(|g| {
        g.log.push_back(linha);
        while g.log.len() > MAX_LOG {
            g.log.pop_front();
        }
    });
    avisar();
}

#[cfg(test)]
pub fn log_linhas() -> Vec<String> {
    com(|g| g.log.iter().map(Entrada::texto).collect())
}

/// Log lines with the account kept apart (for the per-account tabs).
pub fn log_entradas() -> Vec<Entrada> {
    com(|g| g.log.iter().cloned().collect())
}

/// Overall status shown by the tray icon.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Agregado {
    /// No active connection (gray).
    Nenhuma,
    /// Some connection connecting/reconnecting/disconnecting (amber).
    Transicao,
    /// There is an active connection and none in transition (green).
    Conectado,
}

pub fn agregar(situacoes: impl Iterator<Item = Situacao>) -> Agregado {
    let mut alguma_conectada = false;
    for s in situacoes {
        if s.em_transicao() {
            return Agregado::Transicao;
        }
        alguma_conectada |= s == Situacao::Conectado;
    }
    if alguma_conectada {
        Agregado::Conectado
    } else {
        Agregado::Nenhuma
    }
}

/// Summary of all accounts for the tray: icon status and tooltip lines.
/// `contas` = (id, name) in list order.
pub fn resumo(contas: &[(String, String)]) -> (Agregado, Vec<String>) {
    let estados: Vec<(String, EstadoConta)> = contas
        .iter()
        .map(|(id, nome)| (nome.clone(), obter(id)))
        .collect();
    let agregado = agregar(estados.iter().map(|(_, e)| e.situacao));

    let conectadas = estados
        .iter()
        .filter(|(_, e)| e.situacao == Situacao::Conectado)
        .count();
    let mut linhas = vec![match (estados.len(), conectadas) {
        (0, _) => tr!("Nenhuma conta configurada", "No accounts configured").to_string(),
        (_, 0) => tr!("Desconectado", "Disconnected").to_string(),
        (1, 1) => tr!("Conectado", "Connected").to_string(),
        (total, n) => trf!("{n} de {total} conectadas", "{n} of {total} connected"),
    }];
    for (nome, e) in &estados {
        if e.situacao == Situacao::Desconectado {
            continue;
        }
        let detalhe = match (e.situacao, e.trafego) {
            (Situacao::Conectado, Some(t)) => format!(
                "\u{2193} {}  \u{2191} {}",
                crate::fmt_rate(t.down_rate),
                crate::fmt_rate(t.up_rate)
            ),
            _ => e.texto(),
        };
        linhas.push(format!("{nome}: {detalhe}"));
    }
    (agregado, linhas)
}

/// Short HH:MM:SS timestamp via GetLocalTime.
pub fn hora_local() -> String {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTimeW {
        year: u16,
        month: u16,
        dow: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        ms: u16,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetLocalTime(t: *mut SystemTimeW);
    }
    let mut t = SystemTimeW::default();
    unsafe { GetLocalTime(&mut t) };
    format!("{:02}:{:02}:{:02}", t.hour, t.minute, t.second)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: &str) -> String {
        format!("teste-estado-{n}-{}", crate::accounts::novo_id())
    }

    fn t() -> Option<Traffic> {
        Some(Traffic { down_rate: 1.0, up_rate: 1.0, down_total: 10, up_total: 10 })
    }

    #[test]
    fn icone_agregado() {
        use Situacao::*;
        assert_eq!(agregar([].into_iter()), Agregado::Nenhuma);
        assert_eq!(agregar([Desconectado, Desconectado].into_iter()), Agregado::Nenhuma);
        assert_eq!(agregar([Conectado, Desconectado].into_iter()), Agregado::Conectado);
        // a single one in transition is enough to turn the icon amber
        assert_eq!(agregar([Conectado, Reconectando].into_iter()), Agregado::Transicao);
        assert_eq!(agregar([Desconectando].into_iter()), Agregado::Transicao);
    }

    #[test]
    fn sair_de_conectado_limpa_ip_e_trafego() {
        let c = id("limpa");
        definir(&c, Situacao::Conectado, Some("10.0.0.5".into()));
        trafego(&c, t());
        assert!(obter(&c).trafego.is_some());
        assert!(obter(&c).texto().contains("10.0.0.5"));

        definir(&c, Situacao::Reconectando, None);
        let e = obter(&c);
        assert_eq!(e.ip, None);
        assert_eq!(e.trafego, None);
        // late traffic from a connection that dropped does not "revive" the counter
        trafego(&c, t());
        assert_eq!(obter(&c).trafego, None);
        remover(&c);
    }

    #[test]
    fn tooltip_lista_cada_conta_ativa() {
        let (a, b) = (id("a"), id("b"));
        definir(&a, Situacao::Conectado, Some("10.0.0.5".into()));
        definir(&b, Situacao::Desconectado, None);
        let contas = vec![(a.clone(), "Trabalho".to_string()), (b.clone(), "Cliente X".to_string())];
        let (ag, linhas) = resumo(&contas);
        assert_eq!(ag, Agregado::Conectado);
        assert_eq!(linhas[0], "1 de 2 conectadas");
        assert!(linhas.iter().any(|l| l.starts_with("Trabalho:")));
        assert!(!linhas.iter().any(|l| l.starts_with("Cliente X:")));
        remover(&a);
        remover(&b);
    }

    #[test]
    fn log_guarda_a_conta_separada() {
        log("Conta Separada", "linha de teste");
        let e = log_entradas().into_iter().rev().find(|e| e.origem == "Conta Separada").unwrap();
        assert_eq!(e.msg, "linha de teste");
        assert_eq!(e.hora.len(), 8); // HH:MM:SS
        assert!(e.texto().ends_with("[Conta Separada] linha de teste"));
    }

    #[test]
    fn log_prefixa_a_conta() {
        log("Cliente X", "Conectando...");
        assert!(log_linhas().iter().any(|l| l.ends_with("[Cliente X] Conectando...")));
    }
}
