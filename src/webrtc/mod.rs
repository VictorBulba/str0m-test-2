mod socket;
mod state;

use self::state::{LocalPollingState, Track};
use crate::bitrate_measure::{BitrateMeasure};
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
        .clear_codecs()
        // .enable_bwe(Some(Bitrate::mbps(100)))
        .enable_h264(true);

    let mut rtc = rtc_config.build();

    let candidate = Candidate::host(socket.public_addr()).unwrap();
    rtc.add_local_candidate(candidate);
    let answer = rtc.sdp_api().accept_offer(offer).unwrap();

    (rtc, answer)
}

pub(crate) fn start(offer: SdpOffer, public_ip_addr: IpAddr) -> std::io::Result<SdpAnswer> {
    let socket = Socket::new(public_ip_addr)?;

    let (rtc, answer) = make_rtc(offer, &socket);

    std::thread::spawn(move || run_rtc(rtc, socket));

    Ok(answer)
}

#[derive(serde::Serialize, serde::Deserialize)]
struct EncodedFrameDesc {
    data_offset: usize,
    data_len: usize,
    duration: f32,
}

#[derive(Debug)]
struct LoadedEncodedFrame {
    data: Vec<u8>,
    duration: Duration,
}

fn load_encoded_frames() -> Vec<LoadedEncodedFrame> {
    let buf = std::fs::read("./encoded_frames.bin").unwrap();
    let frames_desc: Vec<EncodedFrameDesc> =
        serde_json::from_str(&std::fs::read_to_string("./encoded_frames.json").unwrap()).unwrap();

    let mut encoded_frames = Vec::new();

    for fr in frames_desc {
        encoded_frames.push(LoadedEncodedFrame {
            data: buf[fr.data_offset..(fr.data_offset + fr.data_len)].to_vec(),
            duration: Duration::from_secs_f32(fr.duration),
        });
    }

    encoded_frames
}

#[derive(Debug)]
struct EncodedFrame {
    data: Vec<u8>,
    duration: Duration,
    bitrate: Bitrate,
}

fn start_frames_generator() -> flume::Receiver<EncodedFrame> {
    let (tx, rx) = flume::bounded::<EncodedFrame>(5);

    let encoded_frames = load_encoded_frames();

    let mut bitrate_measure = BitrateMeasure::new(60);

    std::thread::spawn(move || {
        for fr in encoded_frames {
            let sleep_dur = fr.duration;

            let s = Instant::now();

            bitrate_measure.push(fr.data.len() as u32);

            tx.send(EncodedFrame {
                data: fr.data,
                duration: fr.duration,
                bitrate: bitrate_measure.bitrate(),
            }).unwrap();

            let blocking_send_time = s.elapsed();

            std::thread::sleep(sleep_dur.saturating_sub(blocking_send_time));
        }

        println!("No more frames");
        std::process::exit(0);
    });
    rx
}

fn run_rtc(mut rtc: Rtc, socket: Socket) {
    let mut local_state = LocalPollingState::new();

    let frames_rx = start_frames_generator();

    let mut buf = vec![0u8; 2000];

    loop {
        let timeout = match rtc.poll_output().unwrap() {
            Output::Timeout(v) => v,

            Output::Transmit(transmit) => {
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
                        "poll FRAME {} {}",
                        encoded_frame.data.len() as f32 / 1024.0 / 1024.0,
                        encoded_frame.bitrate,
                    );
                    rtc.bwe()
                        .set_current_bitrate(encoded_frame.bitrate);

                    let extra_bitrate = (encoded_frame.bitrate * 0.1)
                        .clamp(Bitrate::kbps(300), Bitrate::mbps(3));
                    let desired_bitrate = Bitrate::from(
                        encoded_frame.bitrate.as_f64() + extra_bitrate.as_f64(),
                    );
                    rtc.bwe().set_desired_bitrate(desired_bitrate);

                    write_frame(&mut rtc, track, &encoded_frame.data, encoded_frame.duration);

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
