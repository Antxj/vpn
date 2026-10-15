//! Windows: the IPv4 routing table and the interface of an address (IP Helper).

use std::net::Ipv4Addr;
use windows_sys::Win32::NetworkManagement::IpHelper::*;
use windows_sys::Win32::Networking::WinSock::{AF_INET, SOCKADDR_INET};

fn ipv4(sa: &SOCKADDR_INET) -> Option<Ipv4Addr> {
    unsafe {
        if sa.si_family != AF_INET {
            return None;
        }
        Some(Ipv4Addr::from(u32::from_be(sa.Ipv4.sin_addr.S_un.S_addr)))
    }
}

/// Routes as (network, prefix length, interface index).
pub fn rotas() -> Vec<(Ipv4Addr, u8, u32)> {
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
        .iter()
        .filter_map(|r| {
            Some((ipv4(&r.DestinationPrefix.Prefix)?, r.DestinationPrefix.PrefixLength, r.InterfaceIndex))
        })
        .collect()
}

/// Index of the interface that has the address.
pub fn interface_do_ip(ip: Ipv4Addr) -> Option<u32> {
    let mut tabela: *mut MIB_UNICASTIPADDRESS_TABLE = std::ptr::null_mut();
    if unsafe { GetUnicastIpAddressTable(AF_INET, &mut tabela) } != 0 || tabela.is_null() {
        return None;
    }
    unsafe {
        let n = (*tabela).NumEntries as usize;
        let linhas = std::slice::from_raw_parts((*tabela).Table.as_ptr(), n);
        let achado = linhas
            .iter()
            .find(|l| ipv4(&l.Address) == Some(ip))
            .map(|l| l.InterfaceIndex);
        FreeMibTable(tabela as *const _);
        achado
    }
}
