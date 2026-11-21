//! Local preview: a worker thread pulls frames from the capture and hands the UI the latest one.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use eframe::egui;
use miroir_capture::{SelectionCancelled, VideoSource};
use miroir_core::frame::{FrameData, VideoFrame};
use tracing::{info, warn};

use crate::fps::FpsCounter;

pub enum Event {
    /// Token that lets the desktop reuse the user's choice next time.
    RestoreToken(String),
    /// The capture is over; the text is the reason when it was not asked for.
    Ended(Option<String>),
}

#[derive(Default)]
struct Shared {
    image: Mutex<Option<egui::ColorImage>>,
    fps: AtomicU32,
    stop: AtomicBool,
}

/// A running capture. Dropping it stops the capture.
pub struct Preview {
    shared: Arc<Shared>,
    events: Receiver<Event>,
}

impl Preview {
    pub fn start(ctx: egui::Context, restore_token: Option<String>) -> Self {
        let shared = Arc::new(Shared::default());
        let (sender, events) = channel();
        // Not joined: it may be waiting for the user in the desktop's picker. It ends by
        // itself once it sees the stop flag.
        std::thread::spawn({
            let shared = shared.clone();
            move || {
                let ended = match run(&ctx, &shared, &sender, restore_token.as_deref()) {
                    Ok(()) => None,
                    Err(e) if e.is::<SelectionCancelled>() => None,
                    Err(e) => {
                        warn!("capture ended: {e:#}");
                        Some(format!("{e:#}"))
                    }
                };
                info!("capture stopped");
                let _ = sender.send(Event::Ended(ended));
                ctx.request_repaint();
            }
        });
        Self { shared, events }
    }

    /// The latest frame, if a new one arrived since the last call.
    pub fn take_image(&self) -> Option<egui::ColorImage> {
        self.shared.image.lock().unwrap().take()
    }

    pub fn fps(&self) -> u32 {
        self.shared.fps.load(Relaxed)
    }

    pub fn poll_event(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }
}

impl Drop for Preview {
    fn drop(&mut self) {
        self.shared.stop.store(true, Relaxed);
    }
}

fn run(
    ctx: &egui::Context,
    shared: &Shared,
    events: &Sender<Event>,
    restore_token: Option<&str>,
) -> anyhow::Result<()> {
    let mut source = open_source(restore_token, events)?;
    let mut fps = FpsCounter::default();
    while !shared.stop.load(Relaxed) {
        let frame = source.next_frame()?;
        let now = Instant::now();
        if let Some(frame) = frame {
            fps.tick(now);
            *shared.image.lock().unwrap() = Some(to_color_image(&frame));
            ctx.request_repaint();
        }
        let current = fps.fps(now);
        if shared.fps.swap(current, Relaxed) != current {
            ctx.request_repaint();
        }
    }
    source.stop();
    Ok(())
}

#[cfg(target_os = "linux")]
fn open_source(
    restore_token: Option<&str>,
    events: &Sender<Event>,
) -> anyhow::Result<Box<dyn VideoSource>> {
    let (source, new_token) = miroir_capture::linux::PortalSource::open(restore_token)?;
    if let Some(token) = new_token {
        let _ = events.send(Event::RestoreToken(token));
    }
    Ok(Box::new(source))
}

#[cfg(not(target_os = "linux"))]
fn open_source(
    _restore_token: Option<&str>,
    _events: &Sender<Event>,
) -> anyhow::Result<Box<dyn VideoSource>> {
    anyhow::bail!("screen capture is not available on this system yet")
}

// ponytail: CPU channel swap plus an egui texture upload per frame, fine for a preview.
// The real viewer draws decoded NV12 with a wgpu shader (phase 2 and later).
pub fn to_color_image(frame: &VideoFrame) -> egui::ColorImage {
    let FrameData::CpuBgra(bgra) = &frame.data;
    let pixels = bgra.chunks_exact(4).map(|p| egui::Color32::from_rgb(p[2], p[1], p[0])).collect();
    egui::ColorImage::new([frame.width as usize, frame.height as usize], pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bgra_becomes_opaque_rgb() {
        let frame = VideoFrame {
            width: 2,
            height: 1,
            pts_us: 0,
            // Blue then red; the alpha byte of a captured frame is not meaningful.
            data: FrameData::CpuBgra(vec![255, 0, 0, 0, 0, 0, 255, 7]),
        };
        let image = to_color_image(&frame);
        assert_eq!(image.size, [2, 1]);
        assert_eq!(image.pixels, [egui::Color32::BLUE, egui::Color32::RED]);
    }
}
