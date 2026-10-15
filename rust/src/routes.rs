//! Routes: full-tunnel and split-tunnel VPNs side by side.
//!
//! - Full tunnel ("all traffic"): the server pushes `redirect-gateway` and
//!   OpenVPN creates the 0.0.0.0/1 + 128.0.0.0/1 routes through the VPN.
//! - Split tunnel ("VPN network only"): only the company networks go through the VPN.
//!
//! One of each at the same time works (the most specific route wins), with
//! one catch: when the full-tunnel VPN connects, the OTHER VPN's traffic to
//! its own server would start going through it - and the other one drops.
//! To avoid that, a full-tunnel VPN is started with one extra route per
//! server of the other connected VPNs, through the local network gateway
//! (`--route <ip> 255.255.255.255 net_gateway`). OpenVPN itself adds those
//! routes (through the interactive service - the app has no admin rights),
//! follows network changes on reconnect and removes them when it disconnects.
//! A split-tunnel VPN connected AFTER the full one simply goes through it.
//!
//! Servers with an internal address are not excluded: they are only
//! reachable through another VPN or through the local network itself.

use std::net::Ipv4Addr;
#[cfg(windows)]
use windows_sys::Win32::NetworkManagement::IpHelper::*;
#[cfg(windows)]
use windows_sys::Win32::Networking::WinSock::{AF_INET, SOCKADDR_INET};

#[cfg(windows)]
fn ipv4(sa: &SOCKADDR_INET) -> Option<Ipv4Addr> {
    unsafe {
        if sa.si_family != AF_INET {
            return None;
        }
        Some(Ipv4Addr::from(u32::from_be(sa.Ipv4.sin_addr.S_un.S_addr)))
    }
}

/// Internet address (the only kind worth keeping on the local network).
/// Internal networks (10/8, 172.16/12, 192.168/16), CGNAT (100.64/10),
/// link-local, loopback etc. are left out.
pub fn publico(ip: Ipv4Addr) -> bool {
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

/// OpenVPN options that keep the given servers outside a full tunnel.
pub fn opcoes_de_exclusao(ips: &[Ipv4Addr]) -> String {
    ips.iter()
        .map(|ip| format!("--route {ip} 255.255.255.255 net_gateway"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Windows IPv4 routing table.
#[cfg(windows)]
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

/// Does the VPN whose adapter has the IP `ip_local` carry all internet
/// traffic? None if the adapter was not found. Only reads the routing table
/// (no administrator rights needed).
#[cfg(windows)]
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

/// Linux: the same answer from /proc/net/route and the interface addresses.
#[cfg(target_os = "linux")]
pub fn tunel_completo(ip_local: &str) -> Option<bool> {
    let ip: Ipv4Addr = ip_local.parse().ok()?;
    let interface = interface_do_ip(ip)?;
    let indice = indice_da_interface(&interface)?;
    let rotas: Vec<(Ipv4Addr, u8, u32)> = rotas_do_proc(&std::fs::read_to_string("/proc/net/route").ok()?)
        .into_iter()
        .filter_map(|(rede, tamanho, nome)| Some((rede, tamanho, indice_da_interface(&nome)?)))
        .collect();
    Some(classificar(&rotas, indice))
}

/// Lines of /proc/net/route -> (network, prefix length, interface). The
/// addresses are hex numbers in the machine's byte order.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn rotas_do_proc(texto: &str) -> Vec<(Ipv4Addr, u8, String)> {
    let endereco = |h: &str| u32::from_str_radix(h, 16).ok().map(|v| Ipv4Addr::from(v.to_le_bytes()));
    texto
        .lines()
        .skip(1)
        .filter_map(|linha| {
            let campos: Vec<&str> = linha.split_whitespace().collect();
            let rede = endereco(campos.get(1)?)?;
            let mascara = endereco(campos.get(7)?)?;
            Some((rede, u32::from(mascara).count_ones() as u8, campos[0].to_string()))
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn indice_da_interface(nome: &str) -> Option<u32> {
    let c = std::ffi::CString::new(nome).ok()?;
    let i = unsafe { libc::if_nametoindex(c.as_ptr()) };
    (i != 0).then_some(i)
}

/// Name of the interface that has the address (getifaddrs).
#[cfg(target_os = "linux")]
fn interface_do_ip(ip: Ipv4Addr) -> Option<String> {
    let mut lista: *mut libc::ifaddrs = std::ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut lista) } != 0 {
        return None;
    }
    let mut achado = None;
    let mut atual = lista;
    while !atual.is_null() {
        let item = unsafe { &*atual };
        if !item.ifa_addr.is_null() && i32::from(unsafe { (*item.ifa_addr).sa_family }) == libc::AF_INET {
            let sin = unsafe { &*(item.ifa_addr as *const libc::sockaddr_in) };
            if Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr)) == ip {
                achado = Some(unsafe { std::ffi::CStr::from_ptr(item.ifa_name) }.to_string_lossy().into_owned());
                break;
            }
        }
        atual = item.ifa_next;
    }
    unsafe { libc::freeifaddrs(lista) };
    achado
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

    fn ip(a: u8, b: u8, c: u8, d: u8) -> Ipv4Addr {
        Ipv4Addr::new(a, b, c, d)
    }

    #[test]
    fn so_exclui_enderecos_da_internet() {
        assert!(publico(ip(200, 160, 2, 3)));
        assert!(publico(ip(8, 8, 8, 8)));
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
            assert!(!publico(interno), "{interno} should not be excluded");
        }
        assert!(publico(ip(100, 63, 0, 1)) && publico(ip(100, 128, 0, 1)));
        assert!(publico(ip(172, 32, 0, 1)));
    }

    #[test]
    fn opcoes_para_o_openvpn() {
        assert_eq!(opcoes_de_exclusao(&[]), "");
        assert_eq!(
            opcoes_de_exclusao(&[ip(200, 1, 2, 3), ip(8, 8, 4, 4)]),
            "--route 200.1.2.3 255.255.255.255 net_gateway --route 8.8.4.4 255.255.255.255 net_gateway"
        );
    }

    #[test]
    fn classifica_tunel_completo_e_dividido() {
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

    #[test]
    fn le_a_tabela_de_rotas_do_linux() {
        let texto = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
                     eth0\t00000000\t0100A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n\
                     tun0\t00000000\t0108000A\t0003\t0\t0\t0\t00000080\t0\t0\t0\n\
                     tun0\t00000080\t0108000A\t0003\t0\t0\t0\t00000080\t0\t0\t0\n\
                     tun0\t0000080A\t00000000\t0001\t0\t0\t0\t00FFFFFF\t0\t0\t0\n";
        let rotas = rotas_do_proc(texto);
        assert_eq!(rotas[0], (ip(0, 0, 0, 0), 0, "eth0".to_string()));
        assert_eq!(rotas[1], (ip(0, 0, 0, 0), 1, "tun0".to_string()));
        assert_eq!(rotas[2], (ip(128, 0, 0, 0), 1, "tun0".to_string()));
        assert_eq!(rotas[3], (ip(10, 8, 0, 0), 24, "tun0".to_string()));
    }

    /// Classifies a really connected VPN (only reads the routing table):
    /// VPN_TEST_IP=<adapter ip> VPN_TEST_FULL=yes|no \
    /// cargo test -- --ignored vpn_real
    #[test]
    #[ignore]
    fn classifica_uma_vpn_real() {
        let ip = std::env::var("VPN_TEST_IP").expect("set VPN_TEST_IP");
        let esperado = std::env::var("VPN_TEST_FULL").expect("set VPN_TEST_FULL") == "yes";
        assert_eq!(tunel_completo(&ip), Some(esperado));
    }
}
