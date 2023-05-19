mod encoder;
mod webrtc;

use axum::extract::Json;
use axum::response::Html;
use axum::routing::{get, post};
use axum::Router;
use std::net::IpAddr;
use str0m::change::{SdpAnswer, SdpOffer};

async fn new_session(offer: SdpOffer, public_ip: IpAddr) -> SdpAnswer {
    let answer = webrtc::start(offer, public_ip).unwrap();

    answer
}

pub async fn run_server(public_ip: IpAddr) {
    let app = Router::new()
        .route(
            "/session",
            post(move |Json(offer): Json<SdpOffer>| async move {
                let answer = new_session(offer, public_ip).await;
                Json(answer)
            }),
        )
        .route(
            "/",
            get(|| async { Html(include_str!("index.html").to_string()) }),
        );

    axum::Server::bind(&"0.0.0.0:4500".parse().unwrap())
        .serve(app.into_make_service())
        .await
        .unwrap();
}

pub fn select_host_address() -> IpAddr {
    use systemstat::Platform as _;
    let system = systemstat::System::new();
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

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let ip = select_host_address();

    println!("IP: {}", ip);

    run_server(ip).await;
}
