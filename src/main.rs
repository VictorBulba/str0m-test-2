mod bitrate_measure;
mod webrtc;

use std::net::IpAddr;
use str0m::change::SdpOffer;

// Handle a web request.
fn web_request(request: &rouille::Request) -> rouille::Response {
    if request.method() == "GET" {
        return rouille::Response::html(include_str!("index.html"));
    }

    // Expected POST SDP Offers.
    let mut data = request.data().expect("body to be available");

    let offer: SdpOffer = serde_json::from_reader(&mut data).expect("serialized offer");

    let public_ip_addr = select_host_address();

    println!("IP: {}", public_ip_addr);

    let answer = webrtc::start(offer, public_ip_addr).unwrap();

    let body = serde_json::to_vec(&answer).expect("answer to serialize");

    rouille::Response::from_data("application/json", body)
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

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let server =
        rouille::Server::new("0.0.0.0:3000", web_request).expect("starting the web server");

    tracing::info!("Listening on {:?}", server.server_addr().port());

    server.run();
}
