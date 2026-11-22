//! Local loop: one thread captures and encodes, as the sharing PC will; another decodes and
//! hands the UI the latest picture, as the viewing PC will.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc::{
    Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError, channel, sync_channel,
};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context as _, anyhow};
use eframe::egui;
use miroir_capture::{SelectionCancelled, VideoSource};
use miroir_codec::{Decoder, EncodedFrame, Encoder};
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
    /// `Ended` was handed out: there is nothing more to report.
    ended: Cell<bool>,
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
        Self {
            shared,
            events,
            ended: Cell::new(false),
        }
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
        if self.ended.get() {
            return None;
        }
        let event = match self.events.try_recv() {
            Ok(event) => event,
            Err(TryRecvError::Empty) => return None,
            // The worker always sends `Ended` before it returns: without it, it panicked.
            Err(TryRecvError::Disconnected) => {
                Event::Ended(Some("erreur interne, voir le terminal".into()))
            }
        };
        self.ended.set(matches!(event, Event::Ended(_)));
        Some(event)
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
    let result = pipeline(&mut *source, ctx, shared);
    source.stop();
    result
}

/// An encoded frame on its way to the display side, with the time it took to encode.
type Packet = (EncodedFrame, Duration);

/// Encoded frames that may wait for the display side. When it is full the sharing side waits,
/// and the capture skips frames meanwhile: never an encoded one, which later frames build on.
const QUEUE: usize = 4;

/// Runs both sides until the capture stops or one of them fails.
///
/// Two threads because each side has a whole frame time to itself on its own PC. In 1440p,
/// encoding takes 11 ms and decoding plus display 9 ms: one after the other they would not fit
/// in the 16.7 ms of a frame at 60 fps.
fn pipeline(
    source: &mut dyn VideoSource,
    ctx: &egui::Context,
    shared: &Shared,
) -> anyhow::Result<()> {
    let (packets, received) = sync_channel(QUEUE);
    std::thread::scope(|scope| {
        let viewer = scope.spawn(move || view(ctx, shared, received));
        // Returning drops the channel's sending end, which is what ends the other side.
        let sharing = share(source, shared, packets);
        let viewing = viewer
            .join()
            .unwrap_or_else(|_| Err(anyhow!("the display side panicked")));
        // If the display side failed, the sharing side only saw a closed channel.
        viewing.and(sharing)
    })
}

/// The sharing side: captures and encodes.
fn share(
    source: &mut dyn VideoSource,
    shared: &Shared,
    packets: SyncSender<Packet>,
) -> anyhow::Result<()> {
    // For tests and diagnostics, e.g. MIROIR_ENCODER=libx264 to try the software fallback.
    let forced = std::env::var("MIROIR_ENCODER").ok();
    let mut encoder: Option<Encoder> = None;
    while !shared.stop.load(Relaxed) {
        let Some(frame) = source.next_frame()? else {
            continue;
        };
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
        let Some(encoded) = encoder.encode(&frame)? else {
            continue;
        };
        if packets.send((encoded, start.elapsed())).is_err() {
            // The display side is gone, and reports why itself.
            break;
        }
    }
    Ok(())
}

/// The display side: decodes, and hands the UI the latest picture and the timings.
fn view(ctx: &egui::Context, shared: &Shared, packets: Receiver<Packet>) -> anyhow::Result<()> {
    let mut decoder = Decoder::new()?;
    let mut stats = Stats::default();
    let mut logged = Instant::now();
    loop {
        // Not forever: the frame rate shown must fall when frames stop coming.
        match packets.recv_timeout(Duration::from_millis(100)) {
            Ok((packet, encode)) => {
                let start = Instant::now();
                if let Some(decoded) = decoder.decode(&packet)? {
                    let decoded_at = Instant::now();
                    let rgba = decoder.to_rgba(&decoded)?;
                    let size = [rgba.width as usize, rgba.height as usize];
                    *shared.image.lock().unwrap() = Some(to_color_image(size, rgba.rows()));
                    ctx.request_repaint();
                    let now = Instant::now();
                    let (decode, display) = (decoded_at - start, now - decoded_at);
                    stats.record(
                        now,
                        Timing {
                            encode,
                            decode,
                            display,
                        },
                    );
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
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

    /// The worker always says why it ends. A channel closed without that word means it
    /// panicked: the UI must not keep showing a frozen picture as if all was well.
    #[test]
    fn a_worker_that_died_ends_the_capture() {
        let (sender, events) = channel();
        let preview = Preview {
            shared: Arc::default(),
            events,
            ended: Cell::new(false),
        };
        assert!(
            preview.poll_event().is_none(),
            "still running: nothing to report"
        );
        drop(sender);
        assert!(matches!(preview.poll_event(), Some(Event::Ended(Some(_)))));
        assert!(preview.poll_event().is_none(), "reported once");
    }

    #[test]
    fn a_normal_end_is_not_mistaken_for_a_crash() {
        let (sender, events) = channel();
        let preview = Preview {
            shared: Arc::default(),
            events,
            ended: Cell::new(false),
        };
        sender.send(Event::Ended(None)).unwrap();
        drop(sender);
        assert!(matches!(preview.poll_event(), Some(Event::Ended(None))));
        assert!(preview.poll_event().is_none());
    }

    /// Gives a few red pictures, then fails like a source that went away.
    struct FewFrames(u64);

    impl VideoSource for FewFrames {
        fn next_frame(&mut self) -> anyhow::Result<Option<miroir_core::frame::VideoFrame>> {
            use miroir_core::frame::{FrameData, VideoFrame};
            anyhow::ensure!(self.0 > 0, "the source is gone");
            self.0 -= 1;
            let data = FrameData::CpuBgra([0, 0, 255, 255].repeat(64 * 48));
            Ok(Some(VideoFrame {
                width: 64,
                height: 48,
                pts_us: 1000 - self.0,
                data,
            }))
        }

        fn stop(&mut self) {}
    }

    #[test]
    fn frames_go_through_both_sides_and_a_failure_is_reported() {
        let shared = Shared::default();
        let result = pipeline(&mut FewFrames(3), &egui::Context::default(), &shared);

        assert_eq!(result.unwrap_err().to_string(), "the source is gone");
        // What was encoded before the failure still reached the display side.
        let image = shared
            .image
            .lock()
            .unwrap()
            .take()
            .expect("no picture was shown");
        assert_eq!(image.size, [64, 48]);
        let [r, g, b, _] = image.pixels[0].to_array();
        assert!(r > 200 && g < 60 && b < 60, "expected red, got {r},{g},{b}");
        assert!(shared.encoder.lock().unwrap().is_some());
    }

    #[test]
    fn stopping_ends_both_sides() {
        let shared = Shared::default();
        shared.stop.store(true, Relaxed);
        pipeline(&mut FewFrames(u64::MAX), &egui::Context::default(), &shared).unwrap();
    }

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
