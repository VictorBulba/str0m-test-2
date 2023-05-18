use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use str0m::net::{DatagramSend, Transmit};
use tokio::net::UdpSocket;

struct DatagramSendIoBuf(DatagramSend);

async fn socket_reader(socket: Arc<UdpSocket>, tx: flume::Sender<(Vec<u8>, SocketAddr)>) {
    let mut buf = vec![0u8; 2000];
    loop {
        let (n, source_addr) = socket.recv_from(&mut buf).await.unwrap();
        buf.truncate(n);
        let result = tx.send((buf.clone(), source_addr));
        if let Err(_) = result {
            tracing::debug!("Socket reader exiting");
            return;
        }
    }
}

async fn socket_writer(socket: Arc<UdpSocket>, rx: flume::Receiver<Transmit>) {
    loop {
        let transmit = match rx.recv_async().await {
            Ok(transmit) => transmit,
            Err(_) => {
                tracing::debug!("Socket writer exiting");
                return;
            }
        };
        socket
            .send_to(&transmit.contents, transmit.destination)
            .await.unwrap();
    }
}

/// A bridge between the tokio-uring
/// and async-capable compute runtimes (such as Carina).
/// 
/// We use it to communicate with IO thread,
/// while performing `rtc` polling on Carina compute runtime.
pub(crate) struct Socket {
    public_addr: SocketAddr,

    write_tx: flume::Sender<Transmit>,
    read_rx: flume::Receiver<(Vec<u8>, SocketAddr)>,
}

impl Socket {
    /// Note: run this on a tokio-uring runtime
    pub(crate) fn new(public_ip_addr: IpAddr) -> std::io::Result<Self> {
        let std_socket = std::net::UdpSocket::bind("0.0.0.0:0")?;
        let local_addr_port = std_socket.local_addr()?.port();
        let public_addr = SocketAddr::new(public_ip_addr, local_addr_port);
        let socket = Arc::new(UdpSocket::from_std(std_socket)?);
        println!("Pub addr: {:?}", public_addr);

        let (read_tx, read_rx) = flume::unbounded();
        tokio::spawn(socket_reader(socket.clone(), read_tx));

        let (write_tx, write_rx) = flume::unbounded();
        tokio::spawn(socket_writer(socket, write_rx));

        Ok(Self {
            public_addr,
            write_tx,
            read_rx,
        })
    }
}

impl Socket {
    pub(crate) fn public_addr(&self) -> SocketAddr {
        self.public_addr
    }

    pub(crate) async fn read(&self) -> (Vec<u8>, SocketAddr) {
        self.read_rx.recv_async().await.unwrap()
    }

    pub(crate) fn write(&self, transmit: Transmit) {
        self.write_tx.send(transmit).unwrap()
    }
}
