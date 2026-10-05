//! Rotas: convivencia entre VPN de tunel completo e de tunel dividido.
//!
//! - Tunel completo ("toda a internet"): o servidor manda `redirect-gateway`
//!   e o OpenVPN cria as rotas 0.0.0.0/1 + 128.0.0.0/1 pela VPN.
//! - Tunel dividido ("so a rede da VPN"): so as redes da empresa vao pela VPN.
//!
//! Um de cada ao mesmo tempo funciona (a rota mais especifica ganha), com um
//! porem: ao ligar a VPN de tunel completo, o trafego da OUTRA VPN ate o
//! proprio servidor passaria a ir por dentro dela - e a outra cai. Para
//! evitar isso, antes de iniciar uma VPN de tunel dividido o app fixa uma
//! rota direta (/32, pelo gateway da rede local) para cada servidor dela.
//! Assim a ordem em que as VPNs sao ligadas deixa de importar.
//!
//! As rotas sao temporarias (somem ao reiniciar o Windows) e sao removidas
//! quando a conexao termina.

use std::net::{Ipv4Addr, ToSocketAddrs};
use std::path::Path;
use windows_sys::Win32::NetworkManagement::IpHelper::*;
use windows_sys::Win32::Networking::WinSock::{AF_INET, MIB_IPPROTO_NETMGMT, SOCKADDR_INET};

const ERROR_OBJECT_ALREADY_EXISTS: u32 = 5010;

/// Hosts dos `remote` do arquivo .ovpn (sem repetir, na ordem do arquivo).
pub fn servidores(texto_ovpn: &str) -> Vec<String> {
    let mut hosts: Vec<String> = Vec::new();
    for linha in texto_ovpn.lines() {
        let linha = linha.trim();
        if linha.starts_with('#') || linha.starts_with(';') {
            continue;
        }
        let mut partes = linha.split_whitespace();
        if partes.next() != Some("remote") {
            continue;
        }
        if let Some(host) = partes.next() {
            if !hosts.iter().any(|h| h.eq_ignore_ascii_case(host)) {
                hosts.push(host.to_string());
            }
        }
    }
    hosts
}

fn ipv4(sa: &SOCKADDR_INET) -> Option<Ipv4Addr> {
    unsafe {
        if sa.si_family != AF_INET {
            return None;
        }
        Some(Ipv4Addr::from(u32::from_be(sa.Ipv4.sin_addr.S_un.S_addr)))
    }
}

fn sockaddr(ip: Ipv4Addr) -> SOCKADDR_INET {
    let mut sa: SOCKADDR_INET = unsafe { std::mem::zeroed() };
    // escrever em campo de union e seguro; so a leitura exige unsafe
    sa.Ipv4.sin_family = AF_INET;
    sa.Ipv4.sin_addr.S_un.S_addr = u32::from(ip).to_be();
    sa
}

/// Tabela de rotas IPv4 do Windows.
fn tabela_de_rotas() -> Vec<MIB_IPFORWARD_ROW2> {
    let mut tabela: *mut MIB_IPFORWARD_TABLE2 = std::ptr::null_mut();
    if unsafe { GetIpForwardTable2(AF_INET, &mut tabela) } != 0 || tabela.is_null() {
        return Vec::new();
    }
    let linhas = unsafe {
        let n = (*tabela).NumEntries as usize;
        std::slice::from_raw_parts((*tabela).Table.as_ptr(), n).to_vec()
    };
    unsafe { FreeMibTable(tabela as *const _) };
    linhas
}

/// Adaptador de VPN (TAP, Wintun, DCO do OpenVPN ou de outro cliente).
fn interface_de_vpn(indice: u32) -> bool {
    let mut linha: MIB_IF_ROW2 = unsafe { std::mem::zeroed() };
    linha.InterfaceIndex = indice;
    if unsafe { GetIfEntry2(&mut linha) } != 0 {
        return false;
    }
    let fim = linha.Description.iter().position(|&c| c == 0).unwrap_or(linha.Description.len());
    let descricao = String::from_utf16_lossy(&linha.Description[..fim]).to_lowercase();
    ["tap-windows", "wintun", "openvpn", "wireguard", "vpn"]
        .iter()
        .any(|v| descricao.contains(v))
}

fn metrica_da_interface(linha: &MIB_IPFORWARD_ROW2) -> u32 {
    let mut iface: MIB_IPINTERFACE_ROW = unsafe { std::mem::zeroed() };
    unsafe { InitializeIpInterfaceEntry(&mut iface) };
    iface.Family = AF_INET;
    iface.InterfaceLuid = linha.InterfaceLuid;
    if unsafe { GetIpInterfaceEntry(&mut iface) } == 0 {
        iface.Metric
    } else {
        0
    }
}

/// Rota padrao da rede local (a "saida normal" para a internet), ignorando
/// adaptadores de VPN.
fn rota_padrao_local() -> Option<MIB_IPFORWARD_ROW2> {
    tabela_de_rotas()
        .into_iter()
        .filter(|r| r.DestinationPrefix.PrefixLength == 0)
        .filter(|r| ipv4(&r.NextHop).is_some_and(|gw| !gw.is_unspecified()))
        .filter(|r| !interface_de_vpn(r.InterfaceIndex))
        .min_by_key(|r| r.Metric.saturating_add(metrica_da_interface(r)))
}

/// Rotas criadas pelo app para uma conexao; removidas no Drop.
pub struct RotasDiretas {
    criadas: Vec<MIB_IPFORWARD_ROW2>,
    pub ips: Vec<Ipv4Addr>,
}

impl Drop for RotasDiretas {
    fn drop(&mut self) {
        for r in &self.criadas {
            unsafe { DeleteIpForwardEntry2(r) };
        }
    }
}

/// Fixa uma rota direta (pela rede local) para cada servidor do .ovpn.
/// Err explica por que nao foi possivel (o app conecta mesmo assim).
pub fn fixar_servidores(config: &Path) -> Result<RotasDiretas, String> {
    let texto = std::fs::read_to_string(config).map_err(|e| e.to_string())?;
    let mut ips: Vec<Ipv4Addr> = Vec::new();
    for host in servidores(&texto) {
        // porta qualquer: so interessa o endereco
        if let Ok(enderecos) = (host.as_str(), 1194).to_socket_addrs() {
            for a in enderecos {
                if let std::net::SocketAddr::V4(v4) = a {
                    let ip = *v4.ip();
                    if !ip.is_loopback() && !ips.contains(&ip) {
                        ips.push(ip);
                    }
                }
            }
        }
    }
    if ips.is_empty() {
        return Err(tr!(
            "não consegui descobrir o endereço do servidor",
            "could not resolve the server address"
        )
        .into());
    }
    let gw = rota_padrao_local().ok_or_else(|| {
        tr!("não encontrei a rota da rede local", "could not find the local network route").to_string()
    })?;

    let mut rotas = RotasDiretas { criadas: Vec::new(), ips: Vec::new() };
    for ip in ips {
        let mut r: MIB_IPFORWARD_ROW2 = unsafe { std::mem::zeroed() };
        unsafe { InitializeIpForwardEntry(&mut r) };
        r.InterfaceLuid = gw.InterfaceLuid;
        r.InterfaceIndex = gw.InterfaceIndex;
        r.DestinationPrefix.Prefix = sockaddr(ip);
        r.DestinationPrefix.PrefixLength = 32;
        r.NextHop = gw.NextHop;
        r.Metric = 1;
        r.Protocol = MIB_IPPROTO_NETMGMT;
        match unsafe { CreateIpForwardEntry2(&r) } {
            0 => {
                rotas.criadas.push(r);
                rotas.ips.push(ip);
            }
            // outra conexao (ou o proprio OpenVPN) ja fixou: nao e nossa
            ERROR_OBJECT_ALREADY_EXISTS => rotas.ips.push(ip),
            _ => {}
        }
    }
    Ok(rotas)
}

/// A VPN cujo adaptador tem o IP `ip_local` manda toda a internet por ela?
/// None se o adaptador nao foi encontrado.
pub fn tunel_completo(ip_local: &str) -> Option<bool> {
    let ip: Ipv4Addr = ip_local.parse().ok()?;
    let mut tabela: *mut MIB_UNICASTIPADDRESS_TABLE = std::ptr::null_mut();
    if unsafe { GetUnicastIpAddressTable(AF_INET, &mut tabela) } != 0 || tabela.is_null() {
        return None;
    }
    let indice = unsafe {
        let n = (*tabela).NumEntries as usize;
        let linhas = std::slice::from_raw_parts((*tabela).Table.as_ptr(), n);
        let achado = linhas
            .iter()
            .find(|l| ipv4(&l.Address) == Some(ip))
            .map(|l| l.InterfaceIndex);
        FreeMibTable(tabela as *const _);
        achado
    }?;
    let rotas: Vec<(Ipv4Addr, u8, u32)> = tabela_de_rotas()
        .iter()
        .filter_map(|r| {
            Some((ipv4(&r.DestinationPrefix.Prefix)?, r.DestinationPrefix.PrefixLength, r.InterfaceIndex))
        })
        .collect();
    Some(classificar(&rotas, indice))
}

/// Tunel completo = a interface tem rota padrao (0.0.0.0/0) ou as duas
/// metades que o OpenVPN usa (0.0.0.0/1 e 128.0.0.0/1, "def1").
fn classificar(rotas: &[(Ipv4Addr, u8, u32)], indice: u32) -> bool {
    let tem = |rede: [u8; 4], tamanho: u8| {
        rotas
            .iter()
            .any(|&(r, t, i)| i == indice && t == tamanho && r == Ipv4Addr::from(rede))
    };
    tem([0, 0, 0, 0], 0) || (tem([0, 0, 0, 0], 1) && tem([128, 0, 0, 0], 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_os_servidores_do_ovpn() {
        let ovpn = "client\n\
                    remote vpn1.exemplo.com.br 1194 udp\n\
                    # remote comentado.exemplo 1194\n\
                    ;remote outro.exemplo 443\n\
                    remote-random\n\
                    <connection>\n  remote 203.0.113.10 443 tcp\n</connection>\n\
                    remote VPN1.exemplo.com.br 443\n\
                    remote-cert-tls server\n";
        assert_eq!(servidores(ovpn), vec!["vpn1.exemplo.com.br", "203.0.113.10"]);
        assert!(servidores("client\ndev tun\n").is_empty());
    }

    #[test]
    fn classifica_tunel_completo_e_dividido() {
        let ip = |a, b, c, d| Ipv4Addr::new(a, b, c, d);
        // tunel completo do OpenVPN (def1) na interface 7
        let completo = vec![
            (ip(0, 0, 0, 0), 0, 3), // rota padrao da rede local
            (ip(0, 0, 0, 0), 1, 7),
            (ip(128, 0, 0, 0), 1, 7),
            (ip(10, 8, 0, 0), 24, 7),
        ];
        assert!(classificar(&completo, 7));
        // tunel dividido: so redes da empresa
        let dividido = vec![(ip(0, 0, 0, 0), 0, 3), (ip(10, 20, 0, 0), 16, 9), (ip(172, 16, 0, 0), 12, 9)];
        assert!(!classificar(&dividido, 9));
        // so uma das metades nao e tunel completo
        assert!(!classificar(&[(ip(0, 0, 0, 0), 1, 9)], 9));
        // rota padrao inteira pela VPN (redirect-gateway sem def1)
        assert!(classificar(&[(ip(0, 0, 0, 0), 0, 9)], 9));
    }

    /// Classifica uma VPN conectada de verdade (so le a tabela de rotas):
    /// VPN_TESTE_IP=<ip do adaptador> VPN_TESTE_COMPLETO=sim|nao \
    /// cargo test -- --ignored vpn_real
    #[test]
    #[ignore]
    fn classifica_uma_vpn_real() {
        let ip = std::env::var("VPN_TESTE_IP").expect("defina VPN_TESTE_IP");
        let esperado = std::env::var("VPN_TESTE_COMPLETO").expect("defina VPN_TESTE_COMPLETO") == "sim";
        assert_eq!(tunel_completo(&ip), Some(esperado));
    }

    /// Mexe na tabela de rotas de verdade (cria e remove uma rota para um
    /// endereco de documentacao). Precisa de administrador:
    /// cargo test -- --ignored rota_direta
    #[test]
    #[ignore]
    fn rota_direta_e_criada_e_removida() {
        let dir = std::env::temp_dir().join(format!("vpn-teste-rotas-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ovpn = dir.join("teste.ovpn");
        std::fs::write(&ovpn, "client\nremote 203.0.113.77 1194\n").unwrap();
        let alvo = Ipv4Addr::new(203, 0, 113, 77);
        let existe = || {
            tabela_de_rotas().iter().any(|r| {
                r.DestinationPrefix.PrefixLength == 32 && ipv4(&r.DestinationPrefix.Prefix) == Some(alvo)
            })
        };
        assert!(!existe());
        {
            let rotas = fixar_servidores(&ovpn).expect("deveria fixar a rota");
            assert_eq!(rotas.ips, vec![alvo]);
            assert!(existe(), "rota nao apareceu na tabela");
        }
        assert!(!existe(), "rota nao foi removida");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
