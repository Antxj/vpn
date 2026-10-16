//! macOS: the IPv4 routing table, read from the kernel with
//! sysctl(NET_RT_DUMP) (what `netstat -rn` shows); the interface of an
//! address comes from unix/routing.rs.

use std::net::Ipv4Addr;

pub use crate::platform::unix::routing::interface_do_ip;

/// Routes as (network, prefix length, interface index).
pub fn rotas() -> Vec<(Ipv4Addr, u8, u32)> {
    let mut mib = [
        libc::CTL_NET,
        libc::PF_ROUTE,
        0,
        libc::AF_INET,
        libc::NET_RT_DUMP,
        0,
    ];
    // the table can grow between the two calls: a few tries
    for _ in 0..3 {
        let mut tamanho: libc::size_t = 0;
        let ok = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                mib.len() as u32,
                std::ptr::null_mut(),
                &mut tamanho,
                std::ptr::null_mut(),
                0,
            )
        };
        if ok != 0 {
            return Vec::new();
        }
        let mut buf = vec![0u8; tamanho + tamanho / 4];
        let mut tamanho = buf.len();
        let ok = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                mib.len() as u32,
                buf.as_mut_ptr().cast(),
                &mut tamanho,
                std::ptr::null_mut(),
                0,
            )
        };
        if ok == 0 {
            buf.truncate(tamanho);
            return rotas_do_kernel(&buf);
        }
    }
    Vec::new()
}

/// Messages of the routing socket -> routes. Each one is an rt_msghdr
/// followed by the addresses flagged in rtm_addrs, each sockaddr rounded up
/// to 4 bytes (a netmask can be shorter than a sockaddr_in: the missing
/// bytes are zero).
fn rotas_do_kernel(buf: &[u8]) -> Vec<(Ipv4Addr, u8, u32)> {
    let cabecalho = std::mem::size_of::<libc::rt_msghdr>();
    let mut saida = Vec::new();
    let mut pos = 0;
    while pos + cabecalho <= buf.len() {
        let msg: libc::rt_msghdr = unsafe { std::ptr::read_unaligned(buf[pos..].as_ptr().cast()) };
        let tamanho = usize::from(msg.rtm_msglen);
        if tamanho < cabecalho || pos + tamanho > buf.len() {
            break;
        }
        if let Some(rota) = rota(&buf[pos + cabecalho..pos + tamanho], &msg) {
            saida.push(rota);
        }
        pos += tamanho;
    }
    saida
}

fn rota(enderecos: &[u8], msg: &libc::rt_msghdr) -> Option<(Ipv4Addr, u8, u32)> {
    let mut destino = None;
    let mut mascara = None;
    let mut pos = 0;
    for bit in 0..libc::RTAX_MAX {
        if msg.rtm_addrs & (1 << bit) == 0 {
            continue;
        }
        let tamanho = usize::from(*enderecos.get(pos)?);
        let sockaddr = enderecos.get(pos..pos + tamanho.max(1))?;
        match bit {
            libc::RTAX_DST => {
                if sockaddr.get(1).map(|f| i32::from(*f)) != Some(libc::AF_INET) {
                    return None;
                }
                destino = Some(ipv4_de(sockaddr));
            }
            libc::RTAX_NETMASK => mascara = Some(ipv4_de(sockaddr)),
            _ => {}
        }
        // each sockaddr takes its length rounded up to 4 (at least 4)
        pos += if tamanho == 0 { 4 } else { (tamanho + 3) & !3 };
    }
    let destino = destino?;
    let prefixo = if msg.rtm_flags & libc::RTF_HOST != 0 {
        32
    } else {
        u32::from(mascara.unwrap_or(Ipv4Addr::UNSPECIFIED)).count_ones() as u8
    };
    Some((destino, prefixo, u32::from(msg.rtm_index)))
}

/// The address of a sockaddr_in (bytes 4..8), with missing bytes as zero.
fn ipv4_de(sockaddr: &[u8]) -> Ipv4Addr {
    let mut b = [0u8; 4];
    for (i, v) in b.iter_mut().enumerate() {
        *v = sockaddr.get(4 + i).copied().unwrap_or(0);
    }
    Ipv4Addr::from(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_a_tabela_de_rotas_do_macos() {
        let rotas = rotas();
        // any Mac with a network (and the CI machine) has a default route
        assert!(
            rotas
                .iter()
                .any(|&(rede, p, i)| rede.is_unspecified() && p == 0 && i > 0),
            "{rotas:?}"
        );
        // and the loopback network
        assert!(
            rotas.iter().any(|&(rede, _, _)| rede.octets()[0] == 127),
            "{rotas:?}"
        );
    }

    #[test]
    fn mascara_curta() {
        // a /16 netmask comes as a 6-byte sockaddr (len, family, port, 255, 255)
        assert_eq!(
            ipv4_de(&[6, 0, 0, 0, 255, 255]),
            Ipv4Addr::new(255, 255, 0, 0)
        );
        assert_eq!(ipv4_de(&[0]), Ipv4Addr::UNSPECIFIED);
    }
}
