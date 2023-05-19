mod socket;
mod state;

use self::state::{LocalPollingState, Track};
use crate::encoder::{EncodedFrame, Encoder};
use socket::Socket;
use std::net::IpAddr;
use std::time::{Duration, Instant};
use str0m::change::{SdpAnswer, SdpOffer};
use str0m::media::MediaTime;
use str0m::net::Receive;
use str0m::Bitrate;
use str0m::{Candidate, Event, Input, Output, Rtc};

pub(crate) fn make_rtc(offer: SdpOffer, socket: &Socket) -> (Rtc, SdpAnswer) {
    let rtc_config = Rtc::builder()
        // .set_ice_lite(true)
        .clear_codecs()
        .enable_vp8(true)
        .enable_bwe(Some(Bitrate::mbps(100)));

    let mut rtc = rtc_config.build();

    let candidate = Candidate::host(socket.public_addr()).unwrap();
    rtc.add_local_candidate(candidate);
    let answer = rtc.sdp_api().accept_offer(offer).unwrap();

    (rtc, answer)
}

pub(crate) fn start(
    offer: SdpOffer,
    public_ip_addr: IpAddr,
    width: u32,
    height: u32,
) -> std::io::Result<SdpAnswer> {
    let socket = Socket::new(public_ip_addr)?;

    let (rtc, answer) = make_rtc(offer, &socket);

    tokio::spawn(run_rtc(rtc, socket, width, height));

    Ok(answer)
}

fn xorshift(state: &mut u32) -> [u8; 3] {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    [
        (*state & 0xFF) as u8,
        ((*state >> 8) & 0xFF) as u8,
        ((*state >> 16) & 0xFF) as u8,
    ]
}

async fn start_frames_generator(width: u32, height: u32) -> tokio::sync::mpsc::UnboundedReceiver<EncodedFrame> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        let pregenerated_frames: Vec<Vec<u8>> = tokio::task::spawn_blocking(move || {
            let mut seed = 283749023;
            // println!("PREGENERATING FRAMES");
            let frames = (0..15)
                .map(|_| {
                    let pixels_count = width * height;
                    let data: Vec<u8> = (0..pixels_count)
                        .flat_map(|_| {
                            let [r, g, b] = xorshift(&mut seed);
                            [r, g, b, 255]
                        })
                        .collect();
                    data
                })
                .collect();
            // println!("DONE PREGENERATING FRAMES");
            frames
        })
        .await
        .unwrap();
    
        let mut encoder = Encoder::new(width, height);
        let frame_dur = Duration::from_millis(20);
        let mut i = 0;
        loop {
            let data = &pregenerated_frames[i % pregenerated_frames.len()];
            i += 1;
            // println!("Encdoing");
            let encoded = encoder.encode(data, frame_dur, Bitrate::ZERO, Instant::now());
            // println!("Done Encdoing");
            tx.send(encoded).unwrap();
            // println!("Sleeping");
            tokio::time::sleep(frame_dur).await;
        }
    });
    rx
}

async fn run_rtc(mut rtc: Rtc, socket: Socket, width: u32, height: u32) {
    let mut local_state = LocalPollingState::new();

    let mut frames_rx = start_frames_generator(width, height).await;

    loop {
        // println!("poll");
        let timeout = match rtc.poll_output().unwrap() {
            Output::Timeout(v) => v,

            Output::Transmit(transmit) => {
                // println!("transmit");
                socket.write(transmit).await;
                continue;
            }

            Output::Event(v) => {
                match v {
                    Event::IceConnectionStateChange(state_change) => {
                        local_state.ice_state_changed(state_change);
                        if local_state.is_closed() {
                            tracing::debug!("Shutting down WebRTC polling loop: disconnected");
                            return;
                        }
                    }
                    Event::MediaAdded(media) => local_state.add_media(media, &mut rtc),
                    Event::ChannelOpen(channel_id, label) => {
                        local_state.add_data_channel(channel_id, label)
                    }
                    Event::ChannelData(channel_data) => match local_state.events_channel {
                        Some(id) if id == channel_data.id => {
                            tracing::trace!("Channel data {:?}", channel_data.data);
                        }
                        _ => (),
                    },
                    Event::EgressBitrateEstimate(bwe) => local_state.set_estimated_bitrate(bwe),
                    _ => (),
                }
                continue;
            }
        };

        let timeout = match timeout.checked_duration_since(Instant::now()) {
            Some(t) => t,
            None => {
                rtc.handle_input(Input::Timeout(Instant::now())).unwrap();
                continue;
            }
        };

        let frame_recv_fut = async {
            if local_state.is_connected() {
                if let Some(track) = local_state.track.as_mut() {
                    println!("poll2");
                    let v = (frames_rx.recv().await, track);
                    println!("poll2.3");
                    return v;
                }
            }
            std::future::pending().await
        };

        // println!("poll1");
        tokio::select! {
            s = socket.read() => {
                // println!("read");
                let (contents, source) = s;
                let input = Input::Receive(
                    Instant::now(),
                    Receive {
                        source,
                        destination: socket.public_addr(),
                        contents: contents.as_slice().try_into().unwrap(),
                    },
                );
                rtc.handle_input(input).unwrap();
                continue;
            },
            encoded_frame = frame_recv_fut => {
                match encoded_frame {
                    (Some(encoded_frame), track) => {
                        println!("GOT FRAME {}", encoded_frame.data().len() as f32 / 1024.0 / 1024.0);
                        rtc.bwe().set_current_bitrate(encoded_frame.current_bitrate());

                        let extra_bitrate = (encoded_frame.current_bitrate() * 0.1).clamp(Bitrate::kbps(300), Bitrate::mbps(3));
                        let desired_bitrate = Bitrate::from(encoded_frame.current_bitrate().as_f64() + extra_bitrate.as_f64());
                        rtc.bwe().set_desired_bitrate(desired_bitrate);

                        write_frame(&mut rtc, track, encoded_frame.data(), encoded_frame.duration());

                        continue;
                    }
                    (None, _) => {
                        panic!("Shutting down WebRTC polling loop");
                    }
                }
            }
            _ = tokio::time::sleep(timeout) => {
                // println!("poll sleep");
            }
        };

        rtc.handle_input(Input::Timeout(Instant::now())).unwrap();
    }
}

fn write_frame(rtc: &mut Rtc, track: &mut Track, frame_data: &[u8], frame_dur: Duration) {
    if !frame_data.is_empty() {
        let mut media = rtc.media(track.mid).unwrap();
        let pt = media.match_params(track.params).unwrap();
        let time = track.accumulated_time;
        track.accumulated_time += frame_dur / 2;
        let media_time: MediaTime = time.into();

        media
            .writer(pt)
            .write(Instant::now(), media_time.rebase(90_000), &frame_data)
            .unwrap();
    }
}
