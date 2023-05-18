mod socket;
mod state;

use self::state::{LocalPollingState, Track, WebrtcSessionState};
use crate::encoder::EncodedFrame;
use crate::GameSession;
use socket::Socket;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use str0m::change::{SdpAnswer, SdpOffer};
use str0m::media::MediaTime;
use str0m::net::Receive;
use str0m::Bitrate;
use str0m::{Candidate, Event, Input, Output, Rtc};

pub(crate) fn make_rtc(offer: SdpOffer, socket: &Socket) -> (Rtc, SdpAnswer) {
    let rtc_config = Rtc::builder()
        .set_ice_lite(true)
        .clear_codecs()
        .enable_h264(true);
    // .enable_bwe(Some(Bitrate::mbps(100)));

    let mut rtc = rtc_config.build();

    let candidate = Candidate::host(socket.public_addr()).unwrap();
    println!("{:?}", candidate);
    rtc.add_local_candidate(candidate);
    let answer = rtc.sdp_api().accept_offer(offer).unwrap();

    (rtc, answer)
}

#[derive(Clone)]
pub(crate) struct WebrtcSession {
    frames_tx: flume::Sender<EncodedFrame>,
    state: Arc<WebrtcSessionState>,
}

impl WebrtcSession {
    /// Note: run this on a tokio-uring runtime
    pub(crate) fn start<T: GameSession>(
        offer: SdpOffer,
        public_ip_addr: IpAddr,
        game_session: Arc<T>,
    ) -> std::io::Result<(Self, SdpAnswer)> {
        let socket = Socket::new(public_ip_addr)?;

        let (rtc, answer) = make_rtc(offer, &socket);

        let inner = Arc::new(WebrtcSessionState::new());

        let (frames_tx, frames_rx) = flume::bounded(1);

        tokio::spawn(run_rtc(
            rtc,
            socket,
            inner.clone(),
            frames_rx,
            game_session,
        ));

        let session = Self {
            frames_tx,
            state: inner,
        };

        Ok((session, answer))
    }

    pub(crate) fn state(&self) -> &WebrtcSessionState {
        &self.state
    }

    pub(crate) fn send_frame(&self, frame: EncodedFrame) -> Result<(), ()> {
        self.frames_tx.send(frame).map_err(|_| ())
    }
}

async fn run_rtc<T: GameSession>(
    mut rtc: Rtc,
    socket: Socket,
    shared_state: Arc<WebrtcSessionState>,
    frames_rx: flume::Receiver<EncodedFrame>,
    game_session: Arc<T>,
) {
    let mut local_state = LocalPollingState::new(shared_state);

    loop {
        let timeout = match rtc.poll_output().unwrap() {
            Output::Timeout(v) => v,

            Output::Transmit(transmit) => {
                println!("{:?}", transmit);
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

        let frame_recv_fut = async {
            if local_state.is_connected() {
                if let Some(track) = local_state.track.as_mut() {
                    return (frames_rx.recv_async().await, track);
                }
            }
            std::future::pending().await
        };

        let mut exit = false;

        tokio::select! {
            s = socket.read() => {
                let (contents, source) = s;
                let input = Input::Receive(
                    Instant::now(),
                    Receive {
                        source,
                        destination: socket.public_addr(),
                        contents: contents.as_slice().try_into().unwrap(),
                    },
                );
                println!("{}", socket.public_addr());
                rtc.handle_input(input).unwrap();
                continue;
            },
            encoded_frame = frame_recv_fut => {
                match encoded_frame {
                    (Ok(encoded_frame), track) => {
                        rtc.bwe().set_current_bitrate(encoded_frame.current_bitrate());

                        let extra_bitrate = (encoded_frame.current_bitrate() * 0.1).clamp(Bitrate::kbps(300), Bitrate::mbps(3));
                        let desired_bitrate = Bitrate::from(encoded_frame.current_bitrate().as_f64() + extra_bitrate.as_f64());
                        rtc.bwe().set_desired_bitrate(desired_bitrate);

                        tracing::trace!("Sending frame (delay: {:?}, size: {})", encoded_frame.time().elapsed(), encoded_frame.data().len());

                        write_frame(&mut rtc, track, encoded_frame.data(), encoded_frame.duration());
                        continue;
                    }
                    (Err(flume::RecvError::Disconnected), _) => {
                        tracing::debug!("Shutting down WebRTC polling loop: session aborted");
                        exit = true; // returns do not work in `select!`
                    }
                }
            }
            _ = tokio::time::sleep(timeout) => {}
        };

        if exit {
            return;
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
