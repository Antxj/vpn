//! One connection per account: asks the OpenVPN interactive service to start
//! openvpn.exe and feeds its management interface with the account's password
//! (freshly generated TOTP token, fixed password or both). Each connection
//! uses its own free management port and writes its own OpenVPN log; the
//! state goes straight to `state`.
//!
//! The app runs without administrator rights: the service performs the
//! privileged network changes (see `service`).

use crate::accounts::Conta;
use crate::service::{self, Falha};
use crate::state::{self, Situacao, Traffic};
use crate::totp;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MAX_AUTH_FAILURES: u32 = 3;

// ---- bookkeeping shared by the connections ---------------------------------

/// openvpn.exe processes started by this app (account id, pid), also saved to
/// disk so the next run can clean up after a crash.
static PROCESSOS: Mutex<Vec<(String, u32)>> = Mutex::new(Vec::new());
/// Server address of each connected account (account id, IP).
static SERVIDORES: Mutex<Vec<(String, Ipv4Addr)>> = Mutex::new(Vec::new());

fn arquivo_de_pids() -> PathBuf {
    crate::dpapi::app_dir().join("openvpn.pids")
}

fn registrar_processo(conta_id: &str, pid: Option<u32>) {
    let mut lista = PROCESSOS.lock().unwrap();
    lista.retain(|(c, _)| c != conta_id);
    if let Some(pid) = pid {
        lista.push((conta_id.to_string(), pid));
    }
    let texto: String = lista.iter().map(|(_, pid)| format!("{pid}\n")).collect();
    let _ = std::fs::create_dir_all(crate::dpapi::app_dir());
    let _ = std::fs::write(arquivo_de_pids(), texto);
}

/// At startup: terminates openvpn.exe processes left behind by a previous run
/// that did not end normally (killed, crashed). Returns how many.
pub fn encerrar_orfaos() -> usize {
    let texto = std::fs::read_to_string(arquivo_de_pids()).unwrap_or_default();
    let encerrados = texto
        .lines()
        .filter_map(|l| l.trim().parse::<u32>().ok())
        .filter(|&pid| service::encerrar_se_openvpn(pid))
        .count();
    let _ = std::fs::remove_file(arquivo_de_pids());
    encerrados
}

fn registrar_servidor(conta_id: &str, ip: Option<Ipv4Addr>) {
    let mut lista = SERVIDORES.lock().unwrap();
    lista.retain(|(c, _)| c != conta_id);
    if let Some(ip) = ip {
        lista.push((conta_id.to_string(), ip));
    }
}

/// Servers of the OTHER connected VPNs, to keep them outside a full tunnel.
fn servidores_de_outras(conta_id: &str) -> Vec<Ipv4Addr> {
    SERVIDORES
        .lock()
        .unwrap()
        .iter()
        .filter(|(c, ip)| c != conta_id && crate::routes::publico(*ip))
        .map(|(_, ip)| *ip)
        .collect()
}

/// Handle of a connection in progress.
pub struct Controller {
    pub stop: Arc<AtomicBool>,
    /// Becomes true when the thread ends (openvpn already exited).
    pub finished: Arc<AtomicBool>,
}

impl Controller {
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::SeqCst)
    }
}

fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(7505)
}

fn mgmt_escape(v: &str) -> String {
    v.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Turns a ">BYTECOUNT:<in>,<out>" line into rates (bytes/s) + totals.
fn parse_bytecount(rest: &str, last: &mut Option<(Instant, u64, u64)>) -> Option<Traffic> {
    let mut it = rest.split(',');
    let down_total: u64 = it.next()?.trim().parse().ok()?;
    let up_total: u64 = it.next()?.trim().parse().ok()?;
    let now = Instant::now();
    let (down_rate, up_rate) = match *last {
        Some((t, di, ui)) => {
            let dt = now.duration_since(t).as_secs_f64().max(0.2);
            (
                down_total.saturating_sub(di) as f64 / dt,
                up_total.saturating_sub(ui) as f64 / dt,
            )
        }
        None => (0.0, 0.0),
    };
    *last = Some((now, down_total, up_total));
    Some(Traffic {
        down_rate,
        up_rate,
        down_total,
        up_total,
    })
}

/// When the OpenVPN log says there is no free network adapter, returns the
/// type (tapctl hwid) to create. Each simultaneous connection needs its own
/// virtual adapter.
fn adaptador_em_falta(log: &str) -> Option<&'static str> {
    let linha = log.lines().find(|l| {
        l.contains("currently in use or disabled") || l.contains("create an adapter")
    })?;
    let l = linha.to_lowercase();
    Some(if l.contains("ovpn-dco") {
        "ovpn-dco"
    } else if l.contains("wintun") {
        "wintun"
    } else {
        "root\\tap0901"
    })
}

/// Error lines from the OpenVPN log, to explain a failure to the user.
fn erros_do_log(log: &str) -> Vec<String> {
    log.lines()
        .filter(|l| {
            let l = l.to_lowercase();
            l.contains("error") || l.contains("fatal") || l.contains("exiting due to")
        })
        .map(|l| l.trim().to_string())
        .collect()
}

fn log_path(conta_id: &str) -> PathBuf {
    crate::dpapi::app_dir()
        .join("logs")
        .join(format!("openvpn-{conta_id}.log"))
}

/// Waits until the token window has enough time left; None if a stop was requested.
fn fresh_token(seed: &str, force_next: bool, stop: &AtomicBool, nome: &str) -> Option<String> {
    let remaining = totp::seconds_remaining();
    if force_next || remaining < totp::MIN_TOKEN_LIFETIME {
        state::log(
            nome,
            trf!(
                "Aguardando próxima janela do token ({remaining}s)...",
                "Waiting for the next token window ({remaining}s)..."
            ),
        );
        let deadline = Instant::now() + Duration::from_millis(remaining * 1000 + 500);
        while Instant::now() < deadline {
            if stop.load(Ordering::SeqCst) {
                return None;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    totp::totp_now(seed)
}

/// Creates another virtual adapter with OpenVPN's own tapctl.exe. Needs
/// administrator rights: Windows asks for them (UAC) at this moment. Rare:
/// with OpenVPN 2.7 the service usually creates the adapter by itself.
fn criar_adaptador(openvpn: &Path, hwid: &str) -> Result<(), String> {
    let tapctl = openvpn.with_file_name("tapctl.exe");
    match crate::elevate::executar_como_admin(&tapctl, &format!("create --hwid {hwid}"))? {
        0 => Ok(()),
        codigo => Err(trf!("o tapctl falhou (código {codigo})", "tapctl failed (code {codigo})")),
    }
}

/// Starts the account's connection in the background.
pub fn start(conta: Conta, openvpn: PathBuf) -> Controller {
    let stop = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    let (stop2, finished2) = (stop.clone(), finished.clone());

    state::definir(&conta.id, Situacao::Conectando, None);
    std::thread::spawn(move || {
        let nome = conta.nome_exibicao().to_string();
        // up to 2 attempts: the second one only happens if a network adapter was missing
        for tentativa in 0..2 {
            let Some(hwid) = run(&conta, &nome, &stop2) else {
                break;
            };
            if tentativa > 0 || stop2.load(Ordering::SeqCst) {
                break;
            }
            state::log(
                &nome,
                tr!(
                    "Todos os adaptadores de rede estão em uso; criando mais um...",
                    "All network adapters are in use; creating another one..."
                ),
            );
            match criar_adaptador(&openvpn, hwid) {
                Ok(()) => state::log(
                    &nome,
                    tr!("Adaptador criado. Conectando de novo...", "Adapter created. Connecting again..."),
                ),
                Err(e) => {
                    state::log(
                        &nome,
                        trf!("Não foi possível criar o adaptador: {e}", "Could not create the adapter: {e}"),
                    );
                    crate::error_box_async(trf!(
                        "{nome}: não há adaptador de rede livre para mais uma conexão \
                         simultânea, e não foi possível criar outro.\n\n{e}",
                        "{nome}: there is no free network adapter for another \
                         simultaneous connection, and another one could not be created.\n\n{e}"
                    ));
                    break;
                }
            }
        }
        // the tray is updated here, not only by the UI: with the window hidden
        // the egui loop does not run and the icon would stay green
        registrar_servidor(&conta.id, None);
        state::definir(&conta.id, Situacao::Desconectado, None);
        finished2.store(true, Ordering::SeqCst);
    });

    Controller { stop, finished }
}

/// Learns from the Windows routes whether the freshly connected VPN carries
/// all internet traffic, and stores it in the account (shown on the card and
/// used by the warning about two full-tunnel VPNs).
fn registrar_tipo_de_tunel(conta: &Conta, nome: &str, ip_local: Option<&str>) {
    let Some(completo) = ip_local.and_then(crate::routes::tunel_completo) else {
        return;
    };
    if crate::engine::get().registrar_tunel(&conta.id, completo) {
        state::log(
            nome,
            if completo {
                tr!(
                    "Esta VPN leva toda a internet (túnel completo).",
                    "This VPN carries all internet traffic (full tunnel)."
                )
            } else {
                tr!(
                    "Esta VPN leva só a rede dela (túnel dividido).",
                    "This VPN carries only its own network (split tunnel)."
                )
            },
        );
    }
}

/// Starts openvpn.exe through the service. When the Windows user is not yet
/// authorized to use the service with any config, offers to authorize it once
/// (an administrator approves it) and tries again.
fn iniciar_pelo_servico(nome: &str, cfg_dir: &Path, opcoes: &str) -> Option<service::Processo> {
    let falhou = |e: &Falha| {
        state::log(nome, trf!("Não consegui iniciar o OpenVPN: {}", "Could not start OpenVPN: {}", e.mensagem()));
        crate::error_box_async(trf!(
            "{nome}: não consegui iniciar o OpenVPN.\n\n{}",
            "{nome}: could not start OpenVPN.\n\n{}",
            e.mensagem()
        ));
    };
    match service::iniciar(cfg_dir, opcoes) {
        Ok(p) => Some(p),
        Err(Falha::NaoAutorizado(msg)) => {
            state::log(nome, msg);
            if cfg!(test) || !perguntar_autorizacao() {
                return None;
            }
            if let Err(e) = service::autorizar_usuario_atual() {
                state::log(nome, trf!("Autorização não concluída: {e}", "Authorization not completed: {e}"));
                return None;
            }
            state::log(nome, tr!("Usuário autorizado no OpenVPN.", "User authorized in OpenVPN."));
            match service::iniciar(cfg_dir, opcoes) {
                Ok(p) => Some(p),
                Err(e) => {
                    falhou(&e);
                    None
                }
            }
        }
        Err(e) => {
            falhou(&e);
            None
        }
    }
}

/// Explains the one-time authorization and asks whether to go ahead.
fn perguntar_autorizacao() -> bool {
    let grupo = service::grupo_autorizado();
    rfd::MessageDialog::new()
        .set_title("VPN")
        .set_description(trf!(
            "Para conectar sem pedir permissão de administrador a cada vez, o \
             OpenVPN precisa autorizar esta conta do Windows uma única vez \
             (incluindo-a no grupo \"{grupo}\").\n\nO Windows vai pedir a \
             permissão de um administrador agora. Continuar?",
            "To connect without asking for administrator permission every time, \
             OpenVPN needs to authorize this Windows account once (adding it to \
             the \"{grupo}\" group).\n\nWindows will ask for an administrator's \
             permission now. Continue?"
        ))
        .set_level(rfd::MessageLevel::Info)
        .set_buttons(rfd::MessageButtons::YesNo)
        .show()
        == rfd::MessageDialogResult::Yes
}

/// Runs one connection attempt. Returns Some(hwid) when OpenVPN exited
/// because no network adapter was free (worth trying again).
fn run(conta: &Conta, nome: &str, stop: &AtomicBool) -> Option<&'static str> {
    let config = PathBuf::from(&conta.config);
    let port = free_port();
    let cfg_dir = config.parent().map(PathBuf::from).unwrap_or_default();
    let log_file = log_path(&conta.id);
    let _ = std::fs::create_dir_all(log_file.parent().unwrap_or(Path::new(".")));
    state::log(
        nome,
        trf!("Iniciando o OpenVPN ({})", "Starting OpenVPN ({})", conta.arquivo()),
    );

    let mut opcoes = vec![
        "--config".to_string(),
        service::argumento(&config.to_string_lossy()),
        "--management 127.0.0.1".to_string(),
        port.to_string(),
        "--management-query-passwords --auth-retry interact --auth-nocache".to_string(),
        "--connect-retry 5 --log".to_string(),
        service::argumento(&log_file.to_string_lossy()),
    ];
    // Full tunnel: the servers of the other connected VPNs stay outside it
    // (through the local network), so those VPNs do not drop when this one
    // takes over the default route. OpenVPN adds and removes these routes.
    if conta.tunel_completo() == Some(true) {
        let outros = servidores_de_outras(&conta.id);
        if !outros.is_empty() {
            opcoes.push(crate::routes::opcoes_de_exclusao(&outros));
            let lista: Vec<String> = outros.iter().map(|ip| ip.to_string()).collect();
            state::log(
                nome,
                trf!(
                    "Servidores de outras VPNs mantidos fora deste túnel: {}",
                    "Servers of other VPNs kept outside this tunnel: {}",
                    lista.join(", ")
                ),
            );
        }
    }
    let Some(processo) = iniciar_pelo_servico(nome, &cfg_dir, &opcoes.join(" ")) else {
        return None;
    };
    registrar_processo(&conta.id, Some(processo.pid));
    if !processo.acompanhavel() {
        state::log(
            nome,
            tr!(
                "Aviso: não consegui acompanhar o processo do OpenVPN.",
                "Warning: could not follow the OpenVPN process."
            ),
        );
    }

    let mut stream: Option<TcpStream> = None;
    let mut buf: Vec<u8> = Vec::new();
    let mut force_next_window = false;
    let mut auth_failures: u32 = 0;
    let mut last_count: Option<(Instant, u64, u64)> = None;
    let mut pedido_de_parada = false;

    loop {
        if processo.terminou().is_some() {
            break;
        }
        if stop.load(Ordering::SeqCst) {
            pedido_de_parada = true;
            state::definir(&conta.id, Situacao::Desconectando, None);
            state::log(nome, tr!("Desconectando...", "Disconnecting..."));
            let mut sent = false;
            if let Some(s) = stream.as_mut() {
                sent = s.write_all(b"signal SIGTERM\r\n").is_ok();
            }
            if !sent {
                processo.matar();
            }
            break;
        }

        let s = match stream.as_mut() {
            Some(s) => s,
            None => {
                match TcpStream::connect_timeout(
                    &([127, 0, 0, 1], port).into(),
                    Duration::from_secs(2),
                ) {
                    Ok(s) => {
                        let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
                        stream = Some(s);
                        let st = stream.as_mut().unwrap();
                        // state on: state events; bytecount 2: traffic every 2 s
                        let _ = st.write_all(b"state on\r\nbytecount 2\r\n");
                        st
                    }
                    Err(_) => {
                        std::thread::sleep(Duration::from_millis(500));
                        continue;
                    }
                }
            }
        };

        let mut chunk = [0u8; 4096];
        match s.read(&mut chunk) {
            Ok(0) => {
                stream = None;
                continue;
            }
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(_) => {
                stream = None;
                continue;
            }
        }

        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let raw: Vec<u8> = buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&raw).trim().to_string();
            if line.is_empty() {
                continue;
            }

            if line.starts_with(">PASSWORD:Need 'Private Key'") {
                // the .ovpn private key is password-protected: not supported
                // yet - without an answer OpenVPN would wait forever
                state::log(
                    nome,
                    tr!(
                        "A chave privada deste .ovpn tem senha, o que o app ainda não suporta.",
                        "This .ovpn private key is password-protected, which the app does not support yet."
                    ),
                );
                crate::error_box_async(trf!(
                    "{nome}: a chave privada do arquivo .ovpn é protegida por senha, \
                     e o app ainda não suporta isso.",
                    "{nome}: the private key in the .ovpn file is password-protected, \
                     which the app does not support yet."
                ));
                stop.store(true, Ordering::SeqCst);
            } else if line.starts_with(">PASSWORD:Need 'Auth'") {
                if !conta.autenticacao.usa_usuario() {
                    // "certificate only" account, but OpenVPN asked anyway
                    crate::error_box_async(trf!(
                        "{nome}: a VPN pediu usuário e senha, mas a conta está como \
                         \"Só certificado\". Edite a conta e escolha a autenticação.",
                        "{nome}: the VPN asked for a username and password, but the account \
                         is set to \"Certificate only\". Edit the account and choose the \
                         authentication."
                    ));
                    stop.store(true, Ordering::SeqCst);
                    continue;
                }
                let token = if conta.autenticacao.usa_token() {
                    let Some(t) = fresh_token(&conta.seed, force_next_window, stop, nome) else {
                        continue;
                    };
                    Some(t)
                } else {
                    None
                };
                force_next_window = false;
                let Some(senha) = conta.compor_senha(token.as_deref()) else {
                    continue;
                };
                state::log(nome, tr!("Enviando usuário e senha...", "Sending username and password..."));
                if let Some(s) = stream.as_mut() {
                    let msg = format!(
                        "username \"Auth\" \"{}\"\r\npassword \"Auth\" \"{}\"\r\n",
                        mgmt_escape(&conta.usuario),
                        mgmt_escape(&senha)
                    );
                    let _ = s.write_all(msg.as_bytes());
                }
            } else if line.starts_with(">PASSWORD:Verification Failed") {
                auth_failures += 1;
                force_next_window = true;
                state::log(
                    nome,
                    trf!(
                        "Autenticação recusada ({auth_failures}x).",
                        "Authentication rejected ({auth_failures}x)."
                    ),
                );
                // a wrong fixed password does not get better by retrying
                let limite = if conta.autenticacao.usa_token() { MAX_AUTH_FAILURES } else { 1 };
                if auth_failures >= limite {
                    let segredo = if conta.autenticacao.usa_token() {
                        "seed"
                    } else {
                        tr!("senha", "password")
                    };
                    crate::error_box_async(trf!(
                        "{nome}: autenticação recusada.\nConfira o usuário e a {segredo} da conta.",
                        "{nome}: authentication rejected.\nCheck the account's username and {segredo}."
                    ));
                    stop.store(true, Ordering::SeqCst);
                }
            } else if let Some(rest) = line.strip_prefix(">BYTECOUNT:") {
                if let Some(traffic) = parse_bytecount(rest, &mut last_count) {
                    state::trafego(&conta.id, Some(traffic));
                }
            } else if let Some(rest) = line.strip_prefix(">STATE:") {
                let parts: Vec<&str> = rest.split(',').collect();
                let state = parts.get(1).unwrap_or(&"").to_string();
                let ip = parts.get(3).filter(|s| !s.is_empty()).map(|s| s.to_string());
                let situacao = state::situacao_do_openvpn(&state);
                if situacao == Situacao::Conectado {
                    auth_failures = 0;
                    registrar_tipo_de_tunel(conta, nome, ip.as_deref());
                    // remote (server) address: 5th field of the CONNECTED state
                    registrar_servidor(&conta.id, parts.get(4).and_then(|s| s.parse().ok()));
                } else {
                    last_count = None;
                    registrar_servidor(&conta.id, None);
                }
                state::definir(&conta.id, situacao, ip);
                state::log(nome, trf!("Estado: {state}", "State: {state}"));
            } else if line.starts_with(">INFO:")
                || line.starts_with("ERROR:")
                || line.starts_with(">FATAL:")
            {
                state::log(nome, line);
            }
        }
    }

    let code = processo.esperar(Duration::from_secs(20));
    registrar_processo(&conta.id, None);
    state::log(
        nome,
        trf!(
            "OpenVPN encerrou (código {}).",
            "OpenVPN exited (code {}).",
            code.map(|c| c.to_string()).unwrap_or_else(|| "?".into())
        ),
    );

    if pedido_de_parada {
        return None;
    }
    // it exited by itself: the OpenVPN log explains why
    let texto_log = std::fs::read_to_string(&log_file).unwrap_or_default();
    if let Some(hwid) = adaptador_em_falta(&texto_log) {
        return Some(hwid);
    }
    for erro in erros_do_log(&texto_log).into_iter().rev().take(3).collect::<Vec<_>>().into_iter().rev() {
        state::log(nome, erro);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytecount_taxas() {
        let mut last = None;
        let t1 = parse_bytecount("1000,500", &mut last).unwrap();
        assert_eq!((t1.down_total, t1.up_total), (1000, 500));
        assert_eq!((t1.down_rate, t1.up_rate), (0.0, 0.0)); // first reading
        std::thread::sleep(Duration::from_millis(250));
        let t2 = parse_bytecount("3000,1500", &mut last).unwrap();
        assert!(t2.down_rate > 0.0 && t2.up_rate > 0.0);
        assert!(t2.down_rate > t2.up_rate);
        // counter reset (reconnection) does not produce a negative rate
        let t3 = parse_bytecount("10,5", &mut last).unwrap();
        assert_eq!((t3.down_rate, t3.up_rate), (0.0, 0.0));
        assert!(parse_bytecount("lixo", &mut last).is_none());
    }

    #[test]
    fn detecta_falta_de_adaptador_pelo_log() {
        let dco = "2026-10-02 10:00:01 All ovpn-dco adapters on this system are currently in use or disabled.\n\
                   2026-10-02 10:00:01 Exiting due to fatal error";
        assert_eq!(adaptador_em_falta(dco), Some("ovpn-dco"));

        let tap = "All tap-windows6 adapters on this system are currently in use or disabled.";
        assert_eq!(adaptador_em_falta(tap), Some("root\\tap0901"));

        let nenhum = "There are no TAP-Windows, Wintun or ovpn-dco adapters on this system. \
                      You should be able to create an adapter by using tapctl.exe utility.";
        assert!(adaptador_em_falta(nenhum).is_some());

        assert_eq!(adaptador_em_falta("TLS handshake failed\nExiting due to fatal error"), None);
    }

    /// Regression (v1.1.1): when the connection ends - including with the window
    /// hidden, when egui is not running - the thread itself sets the account back
    /// to "Disconnected", and only that one (other accounts are not affected).
    #[test]
    fn conta_volta_a_desconectado_quando_a_conexao_termina() {
        let mut conta = Conta::nova();
        conta.nome = "Teste".into();
        let mut outra = Conta::nova();
        outra.nome = "Outra".into();
        state::definir(&conta.id, Situacao::Conectado, Some("10.0.0.5".into()));
        state::definir(&outra.id, Situacao::Conectado, Some("10.0.0.6".into()));

        // non-existent openvpn: the attempt fails and the thread ends
        let ctrl = start(conta.clone(), PathBuf::from(r"C:
ao\existe\openvpn.exe"));
        let limite = Instant::now() + Duration::from_secs(10);
        while Instant::now() < limite && !ctrl.is_finished() {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(ctrl.is_finished(), "the thread did not finish");
        assert_eq!(state::obter(&conta.id).situacao, Situacao::Desconectado);
        assert_eq!(state::obter(&outra.id).situacao, Situacao::Conectado);
        state::remover(&conta.id);
        state::remover(&outra.id);
    }

    #[test]
    fn erros_do_log_explicam_a_falha() {
        let log = "Initialization Sequence Completed\n\
                   ERROR: Cannot resolve host address: vpn.exemplo.com\n\
                   Exiting due to fatal error";
        let e = erros_do_log(log);
        assert_eq!(e.len(), 2);
        assert!(e[0].contains("Cannot resolve"));
    }
}
