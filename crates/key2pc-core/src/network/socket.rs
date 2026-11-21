use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::time::Duration;

use socket2::{Domain, Protocol, Socket, Type};

use crate::error::Result;

#[derive(Debug, Clone)]
pub struct UdpSocketConfig {
    pub bind_addr: SocketAddr,
    pub low_delay_tos: bool,
    pub broadcast: bool,
    pub read_timeout: Option<Duration>,
    pub buffer_size: usize,
}

impl Default for UdpSocketConfig {
    fn default() -> Self {
        Self {
            bind_addr: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)),
            low_delay_tos: true,
            broadcast: true,
            read_timeout: Some(Duration::from_millis(100)),
            buffer_size: 65536,
        }
    }
}

pub fn create_tuned_udp_socket(cfg: &UdpSocketConfig) -> Result<UdpSocket> {
    let domain = Domain::for_address(cfg.bind_addr);
    let socket = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP))?;
    let _ = socket.set_reuse_address(true);
    socket.set_broadcast(cfg.broadcast)?;
    let _ = socket.set_recv_buffer_size(cfg.buffer_size);
    let _ = socket.set_send_buffer_size(cfg.buffer_size);

    if cfg.low_delay_tos && cfg.bind_addr.is_ipv4() && socket.set_tos_v4(0xB8).is_err() {
        let _ = socket.set_tos_v4(0x10);
    }

    disable_udp_connreset(&socket);

    socket.bind(&cfg.bind_addr.into())?;
    let std_socket: UdpSocket = socket.into();
    std_socket.set_read_timeout(cfg.read_timeout)?;
    Ok(std_socket)
}

#[cfg(target_os = "windows")]
fn disable_udp_connreset(socket: &Socket) {
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Networking::WinSock::{WSAIoctl, SIO_UDP_CONNRESET, SOCKET};

    let raw = socket.as_raw_socket() as SOCKET;
    let mut new_behavior: u32 = 0;
    let mut bytes_returned: u32 = 0;
    unsafe {
        WSAIoctl(
            raw,
            SIO_UDP_CONNRESET,
            (&raw mut new_behavior).cast(),
            std::mem::size_of::<u32>() as u32,
            std::ptr::null_mut(),
            0,
            &raw mut bytes_returned,
            std::ptr::null_mut(),
            None,
        );
    }
}

#[cfg(not(target_os = "windows"))]
fn disable_udp_connreset(_socket: &Socket) {}
