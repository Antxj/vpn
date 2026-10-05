//! Uma conexao por conta: lanca o openvpn.exe e alimenta a interface de
//! gerenciamento com a senha da conta (token TOTP recem gerado, senha fixa
//! ou os dois). Cada conexao usa sua propria porta de gerenciamento livre e
//! grava o proprio log do OpenVPN; o estado vai direto para `estado`.

use crate::contas::Conta;
use crate::estado::{self, Situacao, Traffic};
use crate::totp;
use std::ffi::c_void;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const MAX_AUTH_FAILURES: u32 = 3;

// ---- job object: garante que o openvpn.exe morre junto com o app, ------
// ---- mesmo se o app for finalizado a forca (sem daemon orfao) ----------

const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;
const JOB_EXTENDED_LIMIT_CLASS: u32 = 9;

#[repr(C)]
#[derive(Default)]
struct IoCounters {
    read_ops: u64,
    write_ops: u64,
    other_ops: u64,
    read_bytes: u64,
    write_bytes: u64,
    other_bytes: u64,
}

#[repr(C)]
#[derive(Default)]
struct BasicLimits {
    per_process_time: i64,
    per_job_time: i64,
    limit_flags: u32,
    min_ws: usize,
    max_ws: usize,
    active_process_limit: u32,
    affinity: usize,
    priority: u32,
    scheduling: u32,
}

#[repr(C)]
#[derive(Default)]
struct ExtendedLimits {
    basic: BasicLimits,
    io: IoCounters,
    process_mem: usize,
    job_mem: usize,
    peak_process: usize,
    peak_job: usize,
}

#[link(name = "kernel32")]
extern "system" {
    fn CreateJobObjectW(attrs: *mut c_void, name: *const u16) -> *mut c_void;
    fn SetInformationJobObject(job: *mut c_void, class: u32, info: *mut c_void, len: u32) -> i32;
    fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
    fn CloseHandle(handle: *mut c_void) -> i32;
}

struct JobGuard(*mut c_void);
unsafe impl Send for JobGuard {}
impl Drop for JobGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CloseHandle(self.0) };
        }
    }
}

fn attach_kill_job(child: &Child) -> Option<JobGuard> {
    use std::os::windows::io::AsRawHandle;
    unsafe {
        let job = CreateJobObjectW(ptr::null_mut(), ptr::null());
        if job.is_null() {
            return None;
        }
        let mut info = ExtendedLimits::default();
        info.basic.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = SetInformationJobObject(
            job,
            JOB_EXTENDED_LIMIT_CLASS,
            &mut info as *mut _ as *mut c_void,
            std::mem::size_of::<ExtendedLimits>() as u32,
        ) != 0
            && AssignProcessToJobObject(job, child.as_raw_handle() as *mut c_void) != 0;
        if !ok {
            CloseHandle(job);
            return None;
        }
        Some(JobGuard(job))
    }
}

/// Controle de uma conexao em andamento.
pub struct Controller {
    pub stop: Arc<AtomicBool>,
    /// Vira true quando a thread termina (openvpn ja encerrado).
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

/// Converte uma linha ">BYTECOUNT:<in>,<out>" em taxas (bytes/s) + totais.
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

/// Quando o log do OpenVPN diz que nao ha adaptador de rede livre, devolve
/// o tipo (hwid do tapctl) a criar. Cada conexao simultanea precisa do seu
/// proprio adaptador virtual.
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

/// Linhas de erro do log do OpenVPN, para explicar uma falha ao usuario.
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

/// Espera a janela do token ter folga; retorna None se o stop foi pedido.
fn fresh_token(seed: &str, force_next: bool, stop: &AtomicBool, nome: &str) -> Option<String> {
    let remaining = totp::seconds_remaining();
    if force_next || remaining < totp::MIN_TOKEN_LIFETIME {
        estado::log(
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

fn wait_child(child: &mut Child, secs: u64) -> Option<i32> {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if let Ok(Some(status)) = child.try_wait() {
            return status.code();
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let _ = child.kill();
    child.try_wait().ok().flatten().and_then(|s| s.code())
}

/// Cria mais um adaptador virtual com o tapctl.exe do proprio OpenVPN.
fn criar_adaptador(openvpn: &Path, hwid: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let tapctl = openvpn.with_file_name("tapctl.exe");
    let saida = Command::new(&tapctl)
        .args(["create", "--hwid", hwid])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| trf!("não consegui executar o tapctl: {e}", "could not run tapctl: {e}"))?;
    if saida.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&saida.stderr).trim().to_string())
    }
}

/// Inicia a conexao da conta em segundo plano.
pub fn start(conta: Conta, openvpn: PathBuf) -> Controller {
    let stop = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    let (stop2, finished2) = (stop.clone(), finished.clone());

    estado::definir(&conta.id, Situacao::Conectando, None);
    std::thread::spawn(move || {
        let nome = conta.nome_exibicao().to_string();
        // VPN de tunel dividido: o caminho ate o servidor fica fixo na rede
        // local, para nao cair quando outra VPN (de tunel completo) ligar.
        // As rotas vivem ate o fim desta thread (Drop as remove).
        let _rotas = if conta.tunel_completo() == Some(true) {
            None
        } else {
            match crate::rotas::fixar_servidores(Path::new(&conta.config)) {
                Ok(r) => {
                    let ips: Vec<String> = r.ips.iter().map(|ip| ip.to_string()).collect();
                    estado::log(
                        &nome,
                        trf!(
                            "Servidor {} fixado na rede local.",
                            "Server {} pinned to the local network.",
                            ips.join(", ")
                        ),
                    );
                    Some(r)
                }
                Err(e) => {
                    estado::log(&nome, trf!("Sem rota fixa para o servidor: {e}", "No pinned route to the server: {e}"));
                    None
                }
            }
        };
        // ate 2 tentativas: a segunda so acontece se faltou adaptador de rede
        for tentativa in 0..2 {
            let Some(hwid) = run(&conta, &nome, &openvpn, &stop2) else {
                break;
            };
            if tentativa > 0 || stop2.load(Ordering::SeqCst) {
                break;
            }
            estado::log(
                &nome,
                tr!(
                    "Todos os adaptadores de rede estão em uso; criando mais um...",
                    "All network adapters are in use; creating another one..."
                ),
            );
            match criar_adaptador(&openvpn, hwid) {
                Ok(()) => estado::log(
                    &nome,
                    tr!("Adaptador criado. Conectando de novo...", "Adapter created. Connecting again..."),
                ),
                Err(e) => {
                    estado::log(
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
        // a bandeja e atualizada aqui, e nao so pela interface: com a janela
        // oculta o loop do egui nao roda e o icone ficaria verde
        estado::definir(&conta.id, Situacao::Desconectado, None);
        finished2.store(true, Ordering::SeqCst);
    });

    Controller { stop, finished }
}

/// Descobre pelas rotas do Windows se a VPN recem-conectada manda toda a
/// internet por ela, e guarda na conta (mostrado no cartao e usado no aviso
/// de conflito entre duas VPNs de tunel completo).
fn registrar_tipo_de_tunel(conta: &Conta, nome: &str, ip_local: Option<&str>) {
    let Some(completo) = ip_local.and_then(crate::rotas::tunel_completo) else {
        return;
    };
    if crate::motor::get().registrar_tunel(&conta.id, completo) {
        estado::log(
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

/// Executa uma tentativa de conexao. Devolve Some(hwid) quando o OpenVPN
/// encerrou por falta de adaptador de rede livre (vale tentar de novo).
fn run(conta: &Conta, nome: &str, openvpn: &Path, stop: &AtomicBool) -> Option<&'static str> {
    use std::os::windows::process::CommandExt;

    let config = PathBuf::from(&conta.config);
    let port = free_port();
    let cfg_dir = config.parent().map(PathBuf::from).unwrap_or_default();
    let log_file = log_path(&conta.id);
    let _ = std::fs::create_dir_all(log_file.parent().unwrap_or(Path::new(".")));
    estado::log(
        nome,
        trf!("Iniciando o OpenVPN ({})", "Starting OpenVPN ({})", conta.arquivo()),
    );

    let mut child = match Command::new(openvpn)
        .args(["--config"])
        .arg(&config)
        .args([
            "--management",
            "127.0.0.1",
            &port.to_string(),
            "--management-query-passwords",
            "--auth-retry",
            "interact",
            "--auth-nocache",
            "--connect-retry",
            "5",
            "--log",
        ])
        .arg(&log_file)
        .current_dir(&cfg_dir)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            estado::log(
                nome,
                trf!("Não consegui iniciar o OpenVPN: {e}", "Could not start OpenVPN: {e}"),
            );
            crate::error_box_async(trf!(
                "{nome}: não consegui iniciar o OpenVPN.\n\n{e}",
                "{nome}: could not start OpenVPN.\n\n{e}"
            ));
            return None;
        }
    };

    // enquanto este guard existir, o Windows mata o openvpn se o app morrer
    let _job = attach_kill_job(&child);

    let mut stream: Option<TcpStream> = None;
    let mut buf: Vec<u8> = Vec::new();
    let mut force_next_window = false;
    let mut auth_failures: u32 = 0;
    let mut last_count: Option<(Instant, u64, u64)> = None;
    let mut pedido_de_parada = false;

    loop {
        if let Ok(Some(_)) = child.try_wait() {
            break;
        }
        if stop.load(Ordering::SeqCst) {
            pedido_de_parada = true;
            estado::definir(&conta.id, Situacao::Desconectando, None);
            estado::log(nome, tr!("Desconectando...", "Disconnecting..."));
            let mut sent = false;
            if let Some(s) = stream.as_mut() {
                sent = s.write_all(b"signal SIGTERM\r\n").is_ok();
            }
            if !sent {
                let _ = child.kill();
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
                        // state on: eventos de estado; bytecount 2: trafego a cada 2s
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

            if line.starts_with(">PASSWORD:Need 'Auth'") {
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
                estado::log(nome, tr!("Enviando usuário e senha...", "Sending username and password..."));
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
                estado::log(
                    nome,
                    trf!(
                        "Autenticação recusada ({auth_failures}x).",
                        "Authentication rejected ({auth_failures}x)."
                    ),
                );
                // senha fixa errada nao melhora tentando de novo
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
                    estado::trafego(&conta.id, Some(traffic));
                }
            } else if let Some(rest) = line.strip_prefix(">STATE:") {
                let parts: Vec<&str> = rest.split(',').collect();
                let state = parts.get(1).unwrap_or(&"").to_string();
                let ip = parts.get(3).filter(|s| !s.is_empty()).map(|s| s.to_string());
                let situacao = estado::situacao_do_openvpn(&state);
                if situacao == Situacao::Conectado {
                    auth_failures = 0;
                    registrar_tipo_de_tunel(conta, nome, ip.as_deref());
                } else {
                    last_count = None;
                }
                estado::definir(&conta.id, situacao, ip);
                estado::log(nome, trf!("Estado: {state}", "State: {state}"));
            } else if line.starts_with(">INFO:")
                || line.starts_with("ERROR:")
                || line.starts_with(">FATAL:")
            {
                estado::log(nome, line);
            }
        }
    }

    let code = wait_child(&mut child, 20);
    estado::log(
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
    // encerrou sozinho: o log do OpenVPN explica o motivo
    let texto_log = std::fs::read_to_string(&log_file).unwrap_or_default();
    if let Some(hwid) = adaptador_em_falta(&texto_log) {
        return Some(hwid);
    }
    for erro in erros_do_log(&texto_log).into_iter().rev().take(3).collect::<Vec<_>>().into_iter().rev() {
        estado::log(nome, erro);
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
        assert_eq!((t1.down_rate, t1.up_rate), (0.0, 0.0)); // primeira leitura
        std::thread::sleep(Duration::from_millis(250));
        let t2 = parse_bytecount("3000,1500", &mut last).unwrap();
        assert!(t2.down_rate > 0.0 && t2.up_rate > 0.0);
        assert!(t2.down_rate > t2.up_rate);
        // contador reiniciado (reconexao) nao gera taxa negativa
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

    /// Regressao (v1.1.1): quando a conexao termina - inclusive com a janela
    /// oculta, quando o egui nao roda - a propria thread devolve a conta para
    /// "Desconectado", e so ela (as outras contas nao sao afetadas).
    #[test]
    fn conta_volta_a_desconectado_quando_a_conexao_termina() {
        let mut conta = Conta::nova();
        conta.nome = "Teste".into();
        let mut outra = Conta::nova();
        outra.nome = "Outra".into();
        estado::definir(&conta.id, Situacao::Conectado, Some("10.0.0.5".into()));
        estado::definir(&outra.id, Situacao::Conectado, Some("10.0.0.6".into()));

        // openvpn inexistente: a tentativa falha e a thread termina
        let ctrl = start(conta.clone(), PathBuf::from(r"C:
ao\existe\openvpn.exe"));
        let limite = Instant::now() + Duration::from_secs(10);
        while Instant::now() < limite && !ctrl.is_finished() {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(ctrl.is_finished(), "a thread nao terminou");
        assert_eq!(estado::obter(&conta.id).situacao, Situacao::Desconectado);
        assert_eq!(estado::obter(&outra.id).situacao, Situacao::Conectado);
        estado::remover(&conta.id);
        estado::remover(&outra.id);
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
