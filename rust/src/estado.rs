//! Estado compartilhado das conexoes, por conta.
//!
//! As threads de conexao escrevem aqui DIRETAMENTE (nao por canal da
//! interface): com a janela oculta o loop do egui nao roda, e a bandeja
//! precisa refletir quedas e desconexoes mesmo assim. A interface e o timer
//! da bandeja apenas leem.

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

/// Linhas mantidas no log da interface.
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
    /// Em transicao (o icone fica ambar).
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
            Situacao::Desconectado => "Desconectado".into(),
            Situacao::Conectando => "Conectando...".into(),
            Situacao::Reconectando => "Reconectando...".into(),
            Situacao::Desconectando => "Desconectando...".into(),
            Situacao::Conectado => match &self.ip {
                Some(ip) => format!("Conectado  -  IP {ip}"),
                None => "Conectado".into(),
            },
        }
    }
}

#[derive(Default)]
struct Global {
    contas: HashMap<String, EstadoConta>,
    log: VecDeque<String>,
}

type Aviso = Box<dyn Fn() + Send + Sync>;

static GLOBAL: Mutex<Option<Global>> = Mutex::new(None);
static REPINTAR: OnceLock<Aviso> = OnceLock::new();

fn com<R>(f: impl FnOnce(&mut Global) -> R) -> R {
    let mut guard = GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(Global::default))
}

/// Registra quem deve ser avisado a cada mudanca (a interface pede repintura).
pub fn ao_mudar(f: impl Fn() + Send + Sync + 'static) {
    let _ = REPINTAR.set(Box::new(f));
}

fn avisar() {
    if let Some(f) = REPINTAR.get() {
        f();
    }
}

/// Mapeia o estado do OpenVPN (>STATE:) para a situacao da conta.
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

/// Acrescenta uma linha ao log, prefixada com hora e nome da conta.
pub fn log(origem: &str, msg: impl AsRef<str>) {
    let linha = if origem.is_empty() {
        format!("{} {}", hora_local(), msg.as_ref())
    } else {
        format!("{} [{origem}] {}", hora_local(), msg.as_ref())
    };
    com(|g| {
        g.log.push_back(linha);
        while g.log.len() > MAX_LOG {
            g.log.pop_front();
        }
    });
    avisar();
}

pub fn log_linhas() -> Vec<String> {
    com(|g| g.log.iter().cloned().collect())
}

/// Situacao geral exibida no icone da bandeja.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Agregado {
    /// Nenhuma conexao ativa (cinza).
    Nenhuma,
    /// Alguma conexao conectando/reconectando/desconectando (ambar).
    Transicao,
    /// Ha conexao ativa e nenhuma em transicao (verde).
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

/// Resumo de todas as contas para a bandeja: situacao do icone e linhas
/// do tooltip. `contas` = (id, nome) na ordem da lista.
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
        (0, _) => "Nenhuma conta configurada".to_string(),
        (_, 0) => "Desconectado".to_string(),
        (1, 1) => "Conectado".to_string(),
        (total, n) => format!("{n} de {total} conectadas"),
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

/// Timestamp curto HH:MM:SS via GetLocalTime.
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
        format!("teste-estado-{n}-{}", crate::contas::novo_id())
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
        // basta uma em transicao para o icone ficar ambar
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
        // trafego atrasado de uma conexao que caiu nao "revive" o contador
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
    fn log_prefixa_a_conta() {
        log("Cliente X", "Conectando...");
        assert!(log_linhas().iter().any(|l| l.ends_with("[Cliente X] Conectando...")));
    }
}
