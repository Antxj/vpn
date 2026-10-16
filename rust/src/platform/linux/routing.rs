//! Linux: the IPv4 routing table (/proc/net/route); the interface of an
//! address comes from unix/routing.rs.

use crate::platform::unix::routing::indice_da_interface;
use std::net::Ipv4Addr;

pub use crate::platform::unix::routing::interface_do_ip;

/// Routes as (network, prefix length, interface index).
pub fn rotas() -> Vec<(Ipv4Addr, u8, u32)> {
    let texto = std::fs::read_to_string("/proc/net/route").unwrap_or_default();
    rotas_do_proc(&texto)
        .into_iter()
        .filter_map(|(rede, tamanho, nome)| Some((rede, tamanho, indice_da_interface(&nome)?)))
        .collect()
}

/// Lines of /proc/net/route -> (network, prefix length, interface). The
/// addresses are hex numbers in the machine's byte order.
fn rotas_do_proc(texto: &str) -> Vec<(Ipv4Addr, u8, String)> {
    let endereco = |h: &str| {
        u32::from_str_radix(h, 16)
            .ok()
            .map(|v| Ipv4Addr::from(v.to_le_bytes()))
    };
    texto
        .lines()
        .skip(1)
        .filter_map(|linha| {
            let campos: Vec<&str> = linha.split_whitespace().collect();
            let rede = endereco(campos.get(1)?)?;
            let mascara = endereco(campos.get(7)?)?;
            Some((
                rede,
                u32::from(mascara).count_ones() as u8,
                campos[0].to_string(),
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(a: u8, b: u8, c: u8, d: u8) -> Ipv4Addr {
        Ipv4Addr::new(a, b, c, d)
    }

    #[test]
    fn le_a_tabela_de_rotas_do_linux() {
        let texto =
            "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
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
}
