mod encoder;
mod traits;
mod webrtc;

use crate::webrtc::WebrtcSession;
use axum::extract::Json;
use axum::response::Html;
use axum::routing::{get, post};
use axum::Router;
use encoder::Encoder;
use futures::channel::oneshot;
use std::sync::Arc;
use std::time::{Duration, Instant};
use str0m::change::{SdpAnswer, SdpOffer};

pub use traits::{DebugInfo, Frame, Game, GameSession};

pub struct Server<T: Game> {
    game: T,
}

impl<T: Game> Server<T> {
    pub fn new(game: T) -> Self {
        Self { game }
    }

    /// Note: run this on a tokio-uring runtime
    pub async fn run(self) {
        let arc_self = Arc::new(self);

        // let (offer_s, offer_r) = flume::bounded(1);

        // // (for str0m team: i use tokio-uring personally, but removed it for demo purpose)
        // // Unfortunatly, no web servers are compatible with tokio-uring yet,
        // // so we have to communicate with it using channels
        // tokio::spawn(async move {
        //     loop {
        //         println!("recv session request");
        //         let (offer, answer_s): (SdpOffer, oneshot::Sender<SdpAnswer>) =
        //             offer_r.recv_async().await.unwrap();
        //         let answer = arc_self.new_session(offer).await;
        //         let _ = answer_s.send(answer);
        //     }
        // });

        let app = Router::new()
            .route(
                "/session",
                post(move |Json(offer): Json<SdpOffer>| async move {
                    let answer = arc_self.new_session(offer).await;
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

    async fn new_session(&self, offer: SdpOffer) -> SdpAnswer {
        let s = Instant::now();
        let game_session = Arc::new(self.game.new_session(1920, 1080).await);
        tracing::info!("Game session created in {:?}", s.elapsed());

        let (webrtc_session, answer) = WebrtcSession::start(
            offer,
            std::env::var("PUBLIC_IP")
                .expect("Set `PUBLIC_IP` env var")
                .parse()
                .unwrap(),
            game_session.clone(),
            1920,
            1080,
        )
        .unwrap();

        tokio::spawn(game_loop::<T>(webrtc_session, game_session));

        answer
    }
}

async fn game_loop<T: Game>(webrtc_session: WebrtcSession, game_session: Arc<T::Session>) {
    println!("Waiting start");

    tokio::time::sleep(Duration::from_millis(2000)).await;

    // webrtc_session.state().wait_start().await;

    println!("Done waiting start");

    let frames_encoder = FramesEncoder::<T>::start(game_session.clone(), webrtc_session);

    loop {
        let s = Instant::now();

        println!("Frame render");
        let frame = game_session.render_frame().await;
        println!("Frame render dine");

        let wanted_frame_dur = Duration::from_secs_f32(1.0 / 60.0);

        let frame_with_duration = FrameWithDuration {
            frame,
            duration: s.elapsed().max(wanted_frame_dur),
        };
        let result = frames_encoder.send_frame(frame_with_duration);

        if result.is_err() {
            tracing::debug!("Shutting down game loop");
            return;
        }

        let elapsed = s.elapsed();

        if elapsed < wanted_frame_dur {
            tokio::time::sleep(wanted_frame_dur - elapsed).await;
        }
    }
}

struct FrameWithDuration<T: Game> {
    frame: <T::Session as GameSession>::Frame,
    duration: Duration,
}

/// Frames encoding queue.
///
/// To not block the game loop, we send frames to this queue and encode them in a separate task,
/// and when encoding is done, we send the encoded frame to the webrtc session.
struct FramesEncoder<T: Game> {
    frames_tx: flume::Sender<FrameWithDuration<T>>,
}

impl<T: Game> FramesEncoder<T> {
    fn start(game_session: Arc<T::Session>, webrtc_session: WebrtcSession) -> Self {
        let (frames_tx, frames_rx) = flume::unbounded::<FrameWithDuration<T>>();

        let frames_encoder = Self { frames_tx };

        tokio::spawn(async move {
            let mut encoder: Option<Encoder> = None;

            loop {
                let frame = match frames_rx.recv_async().await {
                    Ok(f) => f,
                    Err(flume::RecvError::Disconnected) => {
                        tracing::debug!("Shutting down frames encoder: closed");
                        return;
                    }
                };

                let frame_size = frame.frame.size();

                let mut enc = match encoder.take() {
                    Some(e) if e.size() == frame_size => e,
                    _ => Encoder::new(frame_size.0, frame_size.1),
                };

                let bwe = webrtc_session.state().estimated_bitrate();

                let encoding_s = Instant::now();
                let encoded_frame =
                    enc.encode(frame.frame.data(), frame.duration, bwe, frame.frame.time());
                tracing::trace!("Frame encoded in {:?}", encoding_s.elapsed());

                game_session.send_debug_info(DebugInfo {
                    current: encoded_frame.current_bitrate(),
                    estimated: bwe,
                });

                let result = webrtc_session.send_frame(encoded_frame);

                if result.is_err() {
                    tracing::debug!("Shutting down frames encoder: aborted");
                    return;
                }

                encoder = Some(enc)
            }
        });

        frames_encoder
    }

    fn send_frame(&self, frame: FrameWithDuration<T>) -> Result<(), ()> {
        self.frames_tx.send(frame).map_err(|_| ())
    }
}
