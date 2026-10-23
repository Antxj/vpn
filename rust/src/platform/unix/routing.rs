//! Linux and macOS: network interfaces (getifaddrs, if_nametoindex).

use std::net::Ipv4Addr;

/// Index of a network interface by name (None when it does not exist).
pub fn indice_da_interface(nome: &str) -> Option<u32> {
    let c = std::ffi::CString::new(nome).ok()?;
    let i = unsafe { libc::if_nametoindex(c.as_ptr()) };
    (i != 0).then_some(i)
}

/// Index of the interface that has the address.
pub fn interface_do_ip(ip: Ipv4Addr) -> Option<u32> {
    let mut lista: *mut libc::ifaddrs = std::ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut lista) } != 0 {
        return None;
    }
    let mut achado = None;
    // as_ref: None for a null pointer (the end of the list, an interface
    // without an address)
    let mut atual = unsafe { lista.as_ref() };
    while let Some(item) = atual {
        let address = unsafe { item.ifa_addr.as_ref() };
        if address.is_some_and(|e| i32::from(e.sa_family) == libc::AF_INET) {
            let sin = unsafe { (item.ifa_addr as *const libc::sockaddr_in).as_ref() };
            if sin.is_some_and(|s| Ipv4Addr::from(u32::from_be(s.sin_addr.s_addr)) == ip) {
                achado = Some(
                    unsafe { std::ffi::CStr::from_ptr(item.ifa_name) }
                        .to_string_lossy()
                        .into_owned(),
                );
                break;
            }
        }
        atual = unsafe { item.ifa_next.as_ref() };
    }
    unsafe { libc::freeifaddrs(lista) };
    indice_da_interface(&achado?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interface_of_an_address() {
        // the loopback address is always on an interface (lo / lo0)
        assert!(interface_do_ip(Ipv4Addr::LOCALHOST).is_some());
        // a documentation address (TEST-NET-3) is on none
        assert_eq!(interface_do_ip(Ipv4Addr::new(203, 0, 113, 77)), None);
    }
}
