use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, ToSocketAddrs};

use crate::error::{Key2PcError, Result};

pub fn resolve_target_host(target_host: &str, port: u16) -> Result<Option<SocketAddr>> {
    let trimmed = target_host.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("auto") {
        return Ok(None);
    }

    if let Ok(addr) = trimmed.parse::<SocketAddr>() {
        return Ok(Some(addr));
    }

    if let Ok(ip) = trimmed.parse::<std::net::IpAddr>() {
        return Ok(Some(SocketAddr::new(ip, port)));
    }

    let addrs: Vec<SocketAddr> = (trimmed, port)
        .to_socket_addrs()
        .map_err(|err| {
            Key2PcError::InvalidConfig(format!("Cannot resolve target_host '{trimmed}': {err}"))
        })?
        .collect();

    let chosen = addrs
        .iter()
        .copied()
        .find(|addr| addr.is_ipv4())
        .or_else(|| addrs.first().copied())
        .ok_or_else(|| {
            Key2PcError::InvalidConfig(format!("No IP address resolved for '{trimmed}'"))
        })?;

    Ok(Some(chosen))
}

#[cfg(target_os = "windows")]
pub fn enumerate_broadcast_targets(port: u16) -> Vec<SocketAddr> {
    use socket2::{Domain, Protocol, Socket, Type};
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Networking::WinSock::{
        WSAIoctl, AF_INET, IFF_BROADCAST, IFF_LOOPBACK, IFF_UP, INTERFACE_INFO, SOCKET,
    };

    const SIO_GET_INTERFACE_LIST: u32 = 0x4004_747F;

    let mut targets = vec![SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::BROADCAST, port))];
    let Ok(sock) = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)) else {
        return targets;
    };

    let mut interfaces = [INTERFACE_INFO::default(); 32];
    let mut bytes_returned: u32 = 0;
    let status = unsafe {
        WSAIoctl(
            sock.as_raw_socket() as SOCKET,
            SIO_GET_INTERFACE_LIST,
            std::ptr::null(),
            0,
            interfaces.as_mut_ptr().cast(),
            std::mem::size_of_val(&interfaces) as u32,
            &raw mut bytes_returned,
            std::ptr::null_mut(),
            None,
        )
    };

    if status == 0 {
        let count = (bytes_returned as usize) / std::mem::size_of::<INTERFACE_INFO>();
        for info in interfaces.iter().take(count) {
            let up = (info.iiFlags & IFF_UP) != 0;
            let loopback = (info.iiFlags & IFF_LOOPBACK) != 0;
            let can_broadcast = (info.iiFlags & IFF_BROADCAST) != 0;
            if !up || loopback || !can_broadcast {
                continue;
            }

            let addr_in = unsafe { info.iiAddress.AddressIn };
            let mask_in = unsafe { info.iiNetmask.AddressIn };
            if addr_in.sin_family != AF_INET || mask_in.sin_family != AF_INET {
                continue;
            }

            let ip = u32::from_be(unsafe { addr_in.sin_addr.S_un.S_addr });
            let mask = u32::from_be(unsafe { mask_in.sin_addr.S_un.S_addr });
            let broadcast = Ipv4Addr::from((ip & mask) | !mask);
            let candidate = SocketAddr::V4(SocketAddrV4::new(broadcast, port));
            if !targets.contains(&candidate) {
                targets.push(candidate);
            }
        }
    }

    targets
}

#[cfg(not(target_os = "windows"))]
pub fn enumerate_broadcast_targets(port: u16) -> Vec<SocketAddr> {
    vec![SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::BROADCAST, port))]
}
