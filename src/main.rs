use std::time::Instant;
use server::{Frame, Game, GameSession, Server};
use std::net::IpAddr;
use systemstat::{Platform, System};

pub fn select_host_address() -> IpAddr {
    let system = System::new();
    let networks = system.networks().unwrap();

    for net in networks.values() {
        for n in &net.addrs {
            if let systemstat::IpAddr::V4(v) = n.addr {
                if !v.is_loopback() && !v.is_link_local() && !v.is_broadcast() {
                    return IpAddr::V4(v);
                }
            }
        }
    }

    panic!("Found no usable network interface");
}
struct FrameImpl {
    w: u32,
    h: u32,
    data: Vec<u8>,
    time: Instant,
}

impl Frame for FrameImpl {
    fn data(&self) -> &[u8] {
        self.data()
    }

    fn size(&self) -> (u32, u32) {
        (self.w, self.h)
    }

    fn time(&self) -> Instant {
        self.time
    }
}

struct GameSessionImpl {
    w: u32,
    h: u32,
}

#[async_trait::async_trait]
impl GameSession for GameSessionImpl {
    type Frame = FrameImpl;

    async fn resize(&self, width: u32, height: u32) {
        unimplemented!()
    }

    async fn render_frame(&self) -> Self::Frame {
        let pixels_count = self.w * self.h;
        let data = (0..pixels_count)
            .flat_map(|_| [255u8, 255, 0, 255])
            .collect();
        FrameImpl {
            w: self.w,
            h: self.h,
            data,
            time: Instant::now(),
        }
    }
}

struct GameImpl;

#[async_trait::async_trait]
impl Game for GameImpl {
    type Session = GameSessionImpl;

    async fn new_session(&self, width: u32, height: u32) -> Self::Session {
        GameSessionImpl {
            w: width,
            h: height,
        }
    }
}

#[tokio::main]
async fn main() {
    println!("{}", select_host_address());
    tracing_subscriber::fmt::init();

    Server::new(GameImpl).run().await;
}
