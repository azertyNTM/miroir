//! Local loop: a worker thread captures, encodes, decodes, and hands the UI the latest picture.

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use eframe::egui;
use miroir_capture::{SelectionCancelled, VideoSource};
use miroir_codec::{Decoder, Encoder};
use tracing::{debug, info, warn};

use crate::stats::{Stats, Summary, Timing};

pub enum Event {
    /// Token that lets the desktop reuse the user's choice next time.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    RestoreToken(String),
    /// The capture is over; the text is the reason when it was not asked for.
    Ended(Option<String>),
}

/// The encoder in use: FFmpeg's name for it, and whether the GPU does the work.
pub type EncoderInfo = (&'static str, bool);

#[derive(Default)]
struct Shared {
    image: Mutex<Option<egui::ColorImage>>,
    summary: Mutex<Summary>,
    encoder: Mutex<Option<EncoderInfo>>,
    /// The source is open: the user has answered the desktop's picker.
    open: AtomicBool,
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

    /// The latest decoded picture, if a new one arrived since the last call.
    pub fn take_image(&self) -> Option<egui::ColorImage> {
        self.shared.image.lock().unwrap().take()
    }

    pub fn summary(&self) -> Summary {
        *self.shared.summary.lock().unwrap()
    }

    /// `None` until the first frame is encoded.
    pub fn encoder(&self) -> Option<EncoderInfo> {
        *self.shared.encoder.lock().unwrap()
    }

    /// False while the desktop's picker is still waiting for the user.
    pub fn is_open(&self) -> bool {
        self.shared.open.load(Relaxed)
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
    shared.open.store(true, Relaxed);
    // Also lets the UI save the restore token even if the source sends no frame yet.
    ctx.request_repaint();

    // For tests and diagnostics, e.g. MIROIR_ENCODER=libx264 to try the software fallback.
    let forced = std::env::var("MIROIR_ENCODER").ok();
    let mut encoder: Option<Encoder> = None;
    let mut decoder = Decoder::new()?;
    let mut stats = Stats::default();
    let mut logged = Instant::now();
    while !shared.stop.load(Relaxed) {
        if let Some(frame) = source.next_frame()? {
            // ponytail: a new encoder for every new size, so one per frame while a captured
            // window is being resized. Wait for the size to settle if that ever stutters.
            let size = (frame.width, frame.height);
            let encoder = match &mut encoder {
                Some(encoder) if encoder.source_size() == size => encoder,
                slot => {
                    let new = Encoder::new(size.0, size.1, forced.as_deref())?;
                    *shared.encoder.lock().unwrap() = Some((new.name(), new.is_hardware()));
                    slot.insert(new)
                }
            };

            let start = Instant::now();
            let packet = encoder.encode(&frame)?;
            let encoded = Instant::now();
            let decoded = match &packet {
                Some(packet) => decoder.decode(packet)?,
                None => None,
            };
            let decoded_at = Instant::now();
            if let Some(decoded) = decoded {
                let rgba = decoder.to_rgba(&decoded)?;
                let size = [rgba.width as usize, rgba.height as usize];
                *shared.image.lock().unwrap() = Some(to_color_image(size, rgba.rows()));
                let now = Instant::now();
                let timing = Timing {
                    encode: encoded - start,
                    decode: decoded_at - encoded,
                    display: now - decoded_at,
                };
                stats.record(now, timing);
            }
        }
        let summary = stats.summary(Instant::now());
        if summary.fps > 0 && logged.elapsed() >= Duration::from_secs(1) {
            debug!(?summary, "last second");
            logged = Instant::now();
        }
        if std::mem::replace(&mut *shared.summary.lock().unwrap(), summary) != summary {
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
    // Diagnostics: read a PipeWire node directly, without the desktop's picker.
    if let Ok(node) = std::env::var("MIROIR_PW_NODE") {
        let node = node
            .parse()
            .context("MIROIR_PW_NODE must be a PipeWire node id")?;
        return Ok(Box::new(miroir_capture::linux::PipewireStream::connect(
            None, node,
        )?));
    }
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

/// `rows`: `size[1]` rows of `size[0]` pixels, 4 bytes each: R, G, B and an alpha of 255.
// ponytail: one more copy of the picture, then an egui texture upload, per frame. Both go away
// when the viewer draws the decoder's YUV with its own shader (phase 9).
fn to_color_image<'a>(size: [usize; 2], rows: impl Iterator<Item = &'a [u8]>) -> egui::ColorImage {
    let mut pixels = Vec::with_capacity(size[0] * size[1]);
    for row in rows {
        let row = row.chunks_exact(4);
        pixels.extend(row.map(|p| egui::Color32::from_rgba_premultiplied(p[0], p[1], p[2], p[3])));
    }
    egui::ColorImage::new(size, pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_rows_become_an_image() {
        let rows: [&[u8]; 2] = [
            &[255, 0, 0, 255, 0, 0, 255, 255],
            &[0, 255, 0, 255, 0, 0, 0, 255],
        ];
        let image = to_color_image([2, 2], rows.into_iter());
        assert_eq!(image.size, [2, 2]);
        let expected = [
            egui::Color32::RED,
            egui::Color32::BLUE,
            egui::Color32::GREEN,
            egui::Color32::BLACK,
        ];
        assert_eq!(image.pixels, expected);
    }
}
