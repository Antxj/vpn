//! Routes: full-tunnel and split-tunnel VPNs side by side.
//!
//! - Full tunnel ("all traffic"): the server pushes `redirect-gateway` and
//!   OpenVPN creates the 0.0.0.0/1 + 128.0.0.0/1 routes through the VPN.
//! - Split tunnel ("VPN network only"): only the company networks go through the VPN.
//!
//! One of each at the same time works (the most specific route wins), with
//! one catch: when the full-tunnel VPN connects, the OTHER VPN's traffic to
//! its own server would start going through it - and the other one drops.
//! To avoid that, before starting a split-tunnel VPN the app pins a direct
//! route (/32, through the local network gateway) to each of its servers.
//! That way the order in which the VPNs are connected no longer matters.
//!
//! The routes are temporary (they disappear on a Windows restart), are removed
//! when the connection ends and are recreated if the local network changes
//! while connected (e.g. a laptop moving to another Wi-Fi). Servers with an
//! internal address are not pinned: they are only reachable through another
//! VPN or through the local network itself.

use std::net::{Ipv4Addr, ToSocketAddrs};
use std::path::Path;
use windows_sys::Win32::NetworkManagement::IpHelper::*;
use windows_sys::Win32::Networking::WinSock::{AF_INET, MIB_IPPROTO_NETMGMT, SOCKADDR_INET};

const ERROR_OBJECT_ALREADY_EXISTS: u32 = 5010;

/// Hosts of the .ovpn `remote` lines (no duplicates, in file order).
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
    // writing to a union field is safe; only reading requires unsafe
    sa.Ipv4.sin_family = AF_INET;
    sa.Ipv4.sin_addr.S_un.S_addr = u32::from(ip).to_be();
    sa
}

/// Internet address (the only kind worth pinning to the local network).
/// Internal networks (10/8, 172.16/12, 192.168/16), CGNAT (100.64/10),
/// link-local, loopback etc. are left out.
fn publico(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    let cgnat = a == 100 && (64..=127).contains(&b);
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_multicast()
        || ip.is_broadcast()
        || cgnat)
}

/// Windows IPv4 routing table.
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

/// VPN adapter (TAP, Wintun, OpenVPN DCO or another client's).
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

/// Default route of the local network (the "normal way out" to the
/// internet), ignoring VPN adapters.
fn rota_padrao_local() -> Option<MIB_IPFORWARD_ROW2> {
    tabela_de_rotas()
        .into_iter()
        .filter(|r| r.DestinationPrefix.PrefixLength == 0)
        .filter(|r| ipv4(&r.NextHop).is_some_and(|gw| !gw.is_unspecified()))
        .filter(|r| !interface_de_vpn(r.InterfaceIndex))
        .min_by_key(|r| r.Metric.saturating_add(metrica_da_interface(r)))
}

/// Routes created by the app for one connection; removed on Drop.
pub struct RotasDiretas {
    criadas: Vec<MIB_IPFORWARD_ROW2>,
    pub ips: Vec<Ipv4Addr>,
    /// Gateway used (interface + next hop), to notice a network change.
    gateway: (u64, Option<Ipv4Addr>),
}

impl Drop for RotasDiretas {
    fn drop(&mut self) {
        self.remover();
    }
}

fn chave(gw: &MIB_IPFORWARD_ROW2) -> (u64, Option<Ipv4Addr>) {
    (unsafe { gw.InterfaceLuid.Value }, ipv4(&gw.NextHop))
}

impl RotasDiretas {
    fn remover(&mut self) {
        for r in self.criadas.drain(..) {
            unsafe { DeleteIpForwardEntry2(&r) };
        }
    }

    /// Called when the VPN reconnects: if the local network changed (another
    /// Wi-Fi, cable...), recreates the routes through the new gateway. True if it did.
    pub fn renovar(&mut self) -> bool {
        let Some(gw) = rota_padrao_local() else {
            return false;
        };
        if chave(&gw) == self.gateway {
            return false;
        }
        self.remover();
        let ips = std::mem::take(&mut self.ips);
        let mut novas = criar(&ips, &gw);
        // swap the contents (the Drop of `novas` has nothing left to remove)
        std::mem::swap(self, &mut novas);
        true
    }
}

/// Creates a /32 route to each IP through the given gateway.
fn criar(ips: &[Ipv4Addr], gw: &MIB_IPFORWARD_ROW2) -> RotasDiretas {
    let mut rotas = RotasDiretas { criadas: Vec::new(), ips: Vec::new(), gateway: chave(gw) };
    for &ip in ips {
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
            // another connection (or OpenVPN itself) already pinned it: not ours
            ERROR_OBJECT_ALREADY_EXISTS => rotas.ips.push(ip),
            _ => {}
        }
    }
    rotas
}

/// Pins a direct route (through the local network) to each server of the .ovpn.
/// Err explains why it was not possible (the app connects anyway).
pub fn fixar_servidores(config: &Path) -> Result<RotasDiretas, String> {
    let texto = std::fs::read_to_string(config).map_err(|e| e.to_string())?;
    let mut ips: Vec<Ipv4Addr> = Vec::new();
    for host in servidores(&texto) {
        // any port: only the address matters
        if let Ok(enderecos) = (host.as_str(), 1194).to_socket_addrs() {
            for a in enderecos {
                if let std::net::SocketAddr::V4(v4) = a {
                    let ip = *v4.ip();
                    if publico(ip) && !ips.contains(&ip) {
                        ips.push(ip);
                    }
                }
            }
        }
    }
    if ips.is_empty() {
        return Err(tr!(
            "servidor em rede interna ou endereço não encontrado",
            "server on an internal network or address not found"
        )
        .into());
    }
    let gw = rota_padrao_local().ok_or_else(|| {
        tr!("não encontrei a rota da rede local", "could not find the local network route").to_string()
    })?;

    Ok(criar(&ips, &gw))
}

/// Does the VPN whose adapter has the IP `ip_local` carry all internet
/// traffic? None if the adapter was not found.
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

/// Full tunnel = the interface has the default route (0.0.0.0/0) or both
/// halves OpenVPN uses (0.0.0.0/1 and 128.0.0.0/1, "def1").
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
    fn so_fixa_enderecos_da_internet() {
        let ip = |a, b, c, d| Ipv4Addr::new(a, b, c, d);
        assert!(publico(ip(200, 160, 2, 3)));
        assert!(publico(ip(8, 8, 8, 8)));
        assert!(publico(ip(203, 0, 113, 77))); // documentation range: used in the route test
        for interno in [
            ip(10, 1, 2, 3),
            ip(172, 16, 0, 1),
            ip(172, 31, 255, 254),
            ip(192, 168, 0, 10),
            ip(100, 64, 0, 1),   // CGNAT
            ip(100, 127, 255, 1),
            ip(169, 254, 1, 1),  // link-local
            ip(127, 0, 0, 1),
            ip(0, 0, 0, 0),
        ] {
            assert!(!publico(interno), "{interno} should not be pinned");
        }
        assert!(publico(ip(100, 63, 0, 1)) && publico(ip(100, 128, 0, 1)));
        assert!(publico(ip(172, 32, 0, 1)));
    }

    #[test]
    fn classifica_tunel_completo_e_dividido() {
        let ip = |a, b, c, d| Ipv4Addr::new(a, b, c, d);
        // OpenVPN full tunnel (def1) on interface 7
        let completo = vec![
            (ip(0, 0, 0, 0), 0, 3), // default route of the local network
            (ip(0, 0, 0, 0), 1, 7),
            (ip(128, 0, 0, 0), 1, 7),
            (ip(10, 8, 0, 0), 24, 7),
        ];
        assert!(classificar(&completo, 7));
        // split tunnel: company networks only
        let dividido = vec![(ip(0, 0, 0, 0), 0, 3), (ip(10, 20, 0, 0), 16, 9), (ip(172, 16, 0, 0), 12, 9)];
        assert!(!classificar(&dividido, 9));
        // only one of the halves is not a full tunnel
        assert!(!classificar(&[(ip(0, 0, 0, 0), 1, 9)], 9));
        // whole default route through the VPN (redirect-gateway without def1)
        assert!(classificar(&[(ip(0, 0, 0, 0), 0, 9)], 9));
    }

    /// Classifies a really connected VPN (only reads the routing table):
    /// VPN_TESTE_IP=<adapter ip> VPN_TESTE_COMPLETO=yes|no \
    /// cargo test -- --ignored vpn_real
    #[test]
    #[ignore]
    fn classifica_uma_vpn_real() {
        let ip = std::env::var("VPN_TESTE_IP").expect("set VPN_TESTE_IP");
        let esperado = std::env::var("VPN_TESTE_COMPLETO").expect("set VPN_TESTE_COMPLETO") == "yes";
        assert_eq!(tunel_completo(&ip), Some(esperado));
    }

    /// Touches the real routing table (creates and removes a route to a
    /// documentation address). Needs administrator:
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
            let mut rotas = fixar_servidores(&ovpn).expect("should pin the route");
            assert_eq!(rotas.ips, vec![alvo]);
            assert!(existe(), "route did not show up in the table");
            // same network: nothing to redo
            assert!(!rotas.renovar());
            // simulated network change: redo through the current gateway
            rotas.gateway = (0, None);
            assert!(rotas.renovar());
            assert_eq!(rotas.ips, vec![alvo]);
            assert!(existe(), "route disappeared on renewal");
        }
        assert!(!existe(), "route was not removed");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
