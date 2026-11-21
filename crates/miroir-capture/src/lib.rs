//! Screen and window capture (PipeWire portal on Linux, Windows Graphics Capture on Windows).

use miroir_core::frame::VideoFrame;

#[cfg(target_os = "linux")]
pub mod linux;
pub mod pixels;

/// The user closed the source picker without choosing anything: not a failure.
#[derive(Debug, thiserror::Error)]
#[error("source selection cancelled")]
pub struct SelectionCancelled;

pub trait VideoSource: Send {
    /// Waits a short time for the next frame. `Ok(None)` means nothing new yet: a screen where
    /// nothing moves produces no frames. `Err` means the capture is over.
    fn next_frame(&mut self) -> anyhow::Result<Option<VideoFrame>>;
    fn stop(&mut self);
}
