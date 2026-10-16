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
    let mut atual = lista;
    while !atual.is_null() {
        let item = unsafe { &*atual };
        if !item.ifa_addr.is_null()
            && i32::from(unsafe { (*item.ifa_addr).sa_family }) == libc::AF_INET
        {
            let sin = unsafe { &*(item.ifa_addr as *const libc::sockaddr_in) };
            if Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr)) == ip {
                achado = Some(
                    unsafe { std::ffi::CStr::from_ptr(item.ifa_name) }
                        .to_string_lossy()
                        .into_owned(),
                );
                break;
            }
        }
        atual = item.ifa_next;
    }
    unsafe { libc::freeifaddrs(lista) };
    indice_da_interface(&achado?)
}
