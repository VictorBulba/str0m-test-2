use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use str0m::net::{DatagramSend, Transmit};
use tokio::net::UdpSocket;

pub(crate) struct Socket {
    public_addr: SocketAddr,
    socket: UdpSocket,
}

impl Socket {
    pub(crate) fn new(public_ip_addr: IpAddr) -> std::io::Result<Self> {
        let std_socket = std::net::UdpSocket::bind("0.0.0.0:0")?;
        let local_addr_port = std_socket.local_addr()?.port();
        let public_addr = SocketAddr::new(public_ip_addr, local_addr_port);
        let socket = UdpSocket::from_std(std_socket)?;

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

    pub(crate) async fn read(&self) -> (Vec<u8>, SocketAddr) {
        let mut buf = vec![0u8; 2000];
        let (n, source_addr) = self.socket.recv_from(&mut buf).await.unwrap();
        buf.truncate(n);
        (buf, source_addr)
    }

    pub(crate) async fn write(&self, transmit: Transmit) {
        self.socket
            .send_to(&transmit.contents, transmit.destination)
            .await
            .unwrap();
    }
}
