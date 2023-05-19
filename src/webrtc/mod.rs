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
        .enable_bwe(Some(Bitrate::mbps(100)))
        .enable_vp8(true);

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

    std::thread::spawn(move || run_rtc(rtc, socket, width, height));

    Ok(answer)
}

/// Fancy AI generated function
fn gen_frame(seed: &mut u32, width: u32, height: u32, frame_number: u32) -> Vec<u8> {
    let mut pixels = Vec::new();

    let grid_width = width / 15;
    let grid_height = height / 15;

    // Generate all pixels
    for pixel_i in 0..(width * height) as usize {
        // Simple PRNG
        *seed = seed.wrapping_mul(1103515245).wrapping_add(12345 + frame_number);

        // Calculate the position of the pixel in the image
        let x = pixel_i as u32 % width;
        let y = pixel_i as u32 / width;

        // Calculate the position of the grid cell
        let cell_x = x / grid_width;
        let cell_y = y / grid_height;

        // Generate a distinct color for each grid cell that changes over time
        let r = ((cell_x * 50 + frame_number) % 256) as u8;
        let g = ((cell_y * 50 + frame_number) % 256) as u8;
        let b = (((*seed % 16) + cell_x + cell_y) % 256) as u8;

        pixels.push([r, g, b]);
    }

    pixels.into_iter().flat_map(|[r, g, b]| [r, g, b, 255]).collect()
}

fn start_frames_generator(width: u32, height: u32) -> tokio::sync::mpsc::Receiver<EncodedFrame> {
    let (tx, rx) = tokio::sync::mpsc::channel(5);

    let mut seed = 2932342342;

    let pregenerated_frames: Vec<Vec<u8>> = (0..1200)
        .map(|i| gen_frame(&mut seed, width, height, i))
        .collect();

    std::thread::spawn(move || {
        let mut encoder = Encoder::new(width, height);
        let frame_dur = Duration::from_secs_f32(1.0 / 60.0);

        for i in 0.. {
            let s = Instant::now();
            // let data: Vec<u8> = (0..pixels_count).flat_map(|_| [v, v, v, 255]).collect();
            // let data = gen_gradient_frame(&mut seed, width, height);
            let data = &pregenerated_frames[i % pregenerated_frames.len()];
            // println!("Encdoing");
            let encoded = encoder.encode(&data, frame_dur, Bitrate::ZERO, Instant::now());
            println!("Done Encdoing in {:?}", s.elapsed());

            tx.blocking_send(encoded).unwrap();

            let frame_creating_time = s.elapsed();

            std::thread::sleep(frame_dur.saturating_sub(frame_creating_time));
        }
    });
    rx
}

fn run_rtc(mut rtc: Rtc, socket: Socket, width: u32, height: u32) {
    let mut local_state = LocalPollingState::new();

    let mut frames_rx = start_frames_generator(width, height);

    let mut buf = vec![0u8; 2000];

    loop {
        rtc.bwe().set_current_bitrate(Bitrate::mbps(10));
        rtc.bwe().set_desired_bitrate(Bitrate::mbps(10));
        // println!("poll");
        // let s = Instant::now();
        let timeout = match rtc.poll_output().unwrap() {
            Output::Timeout(v) => v,

            Output::Transmit(transmit) => {
                // println!("transmit");
                socket.write(transmit);
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

        // println!("poll0 {:?}", s.elapsed());

        let timeout = match timeout.checked_duration_since(Instant::now()) {
            Some(t) => t,
            None => {
                rtc.handle_input(Input::Timeout(Instant::now())).unwrap();
                continue;
            }
        };

        if local_state.is_connected() {
            if let Some(track) = local_state.track.as_mut() {
                if let Ok(encoded_frame) = frames_rx.try_recv() {
                    println!(
                        "poll FRAME {} {:?}",
                        encoded_frame.data().len() as f32 / 1024.0 / 1024.0,
                        encoded_frame.time().elapsed()
                    );
                    rtc.bwe()
                        .set_current_bitrate(encoded_frame.current_bitrate());

                    let extra_bitrate = (encoded_frame.current_bitrate() * 0.1)
                        .clamp(Bitrate::kbps(300), Bitrate::mbps(3));
                    let desired_bitrate = Bitrate::from(
                        encoded_frame.current_bitrate().as_f64() + extra_bitrate.as_f64(),
                    );
                    rtc.bwe().set_desired_bitrate(desired_bitrate);

                    write_frame(
                        &mut rtc,
                        track,
                        encoded_frame.data(),
                        encoded_frame.duration(),
                    );

                    continue;
                }
            }
        }

        match socket.read(&mut buf, timeout) {
            Some((n, source)) => {
                let input = Input::Receive(
                    Instant::now(),
                    Receive {
                        source,
                        destination: socket.public_addr(),
                        contents: (&buf[..n]).try_into().unwrap(),
                    },
                );
                rtc.handle_input(input).unwrap();
                continue;
            }
            None => (),
        }

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
