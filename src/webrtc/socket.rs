use std::io::ErrorKind;
use std::net::{IpAddr, SocketAddr};
use str0m::net::Transmit;
use systemstat::Duration;
use std::net::UdpSocket;

pub(crate) struct Socket {
    public_addr: SocketAddr,
    socket: UdpSocket,
}

impl Socket {
    pub(crate) fn new(public_ip_addr: IpAddr) -> std::io::Result<Self> {
        let socket = std::net::UdpSocket::bind("0.0.0.0:0")?;
        let local_addr_port = socket.local_addr()?.port();
        let public_addr = SocketAddr::new(public_ip_addr, local_addr_port);

        Ok(Self {
            public_addr,
            socket,
        })
    }
}

impl Socket {
    pub(crate) fn public_addr(&self) -> SocketAddr {
        self.public_addr
    }

    pub(crate) fn read(&self, buf: &mut Vec<u8>, timeout: Duration) -> Option<(usize, SocketAddr)> {
        self.socket.set_read_timeout(Some(timeout));
        match self.socket.recv_from(buf) {
            Ok(read) => Some(read),
            Err(err) => match err.kind() {
                ErrorKind::WouldBlock | ErrorKind::TimedOut => None,
                err => panic!("{}", err),
            },
        }
    }

    pub(crate) fn write(&self, transmit: Transmit) {
        self.socket
            .send_to(&transmit.contents, transmit.destination)
            .unwrap();
    }
}
