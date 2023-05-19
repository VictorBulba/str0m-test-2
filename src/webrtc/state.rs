use std::time::Duration;
use str0m::channel::ChannelId;
use str0m::format::PayloadParams;
use str0m::media::{MediaAdded, MediaKind, Mid};
use str0m::{Bitrate, IceConnectionState, Rtc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectionState {
    New,
    Connected,
    Closed,
}

pub(crate) struct Track {
    pub(crate) mid: Mid,
    pub(crate) params: PayloadParams,
    pub(crate) accumulated_time: Duration,
}

pub(crate) struct LocalPollingState {
    pub(crate) track: Option<Track>,
    pub(crate) events_channel: Option<ChannelId>,
    state: ConnectionState,
    bwe: Bitrate,
}

impl LocalPollingState {
    pub(crate) fn new() -> Self {
        Self {
            track: None,
            events_channel: None,
            state: ConnectionState::New,
            bwe: Bitrate::ZERO,
        }
    }

    pub(crate) fn add_media(&mut self, media: MediaAdded, rtc: &mut Rtc) {
        tracing::debug!("Media added");

        if self.track.is_some() {
            tracing::error!("Only one track is supported");
            return;
        }

        assert_eq!(media.kind, MediaKind::Video);
        assert!(media.direction.is_sending());

        let m = rtc.media(media.mid).unwrap();
        let params = m.payload_params();

        self.track = Some(Track {
            mid: media.mid,
            params: params[0].clone(),
            accumulated_time: Duration::ZERO,
        });
    }

    pub(crate) fn add_data_channel(&mut self, channel: ChannelId, label: String) {
        tracing::debug!("Data channel {} added", label);

        if self.events_channel.is_some() {
            tracing::error!("Only one data channel is supported");
            return;
        }

        if label == "events" {
            self.events_channel = Some(channel);
        } else {
            tracing::error!("Unknown data channel {}", label);
        }
    }

    pub(crate) fn set_estimated_bitrate(&mut self, bitrate: Bitrate) {
        let clamped_bitrate = bitrate.clamp(Bitrate::ZERO, Bitrate::mbps(20));
        self.bwe = clamped_bitrate;
    }

    pub(crate) fn ice_state_changed(&mut self, new_state: IceConnectionState) {
        match new_state {
            IceConnectionState::Disconnected => {
                tracing::debug!("ICE disconnected, closing WebRTC session");
                self.state = ConnectionState::Closed;
            }
            IceConnectionState::Connected | IceConnectionState::Completed => {
                tracing::debug!("ICE connected");
                self.state = ConnectionState::Connected;
            }
            _ => {}
        }
    }

    pub(crate) fn is_connected(&self) -> bool {
        self.state == ConnectionState::Connected
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.state == ConnectionState::Closed
    }
}
