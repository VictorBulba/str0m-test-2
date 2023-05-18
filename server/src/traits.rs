use std::time::Instant;

use str0m::Bitrate;

#[derive(Debug, Clone, Copy)]
pub struct DebugInfo {
    pub current: Bitrate,
    pub estimated: Bitrate,
}

impl Default for DebugInfo {
    fn default() -> Self {
        Self {
            current: Bitrate::ZERO,
            estimated: Bitrate::ZERO,
        }
    }
}

pub trait Frame: 'static + Send + Sync {
    fn data(&self) -> &[u8];

    fn size(&self) -> (u32, u32);

    /// Time when this frame was captured
    fn time(&self) -> Instant;
}

#[async_trait::async_trait]
pub trait GameSession: 'static + Send + Sync {
    type Frame: Frame;

    async fn resize(&self, width: u32, height: u32);

    async fn render_frame(&self) -> Self::Frame;

    fn send_debug_info(&self, _debug_info: DebugInfo) {
        // Debug info could be ignored
    }
}

#[async_trait::async_trait]
pub trait Game: 'static + Send + Sync {
    type Session: GameSession;

    async fn new_session(&self, width: u32, height: u32) -> Self::Session;
}
