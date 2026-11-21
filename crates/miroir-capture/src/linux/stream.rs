//! PipeWire consumer: a main loop on its own thread that keeps only the latest frame.

use std::os::fd::OwnedFd;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::anyhow;
use miroir_core::frame::{FrameData, VideoFrame};
use pipewire as pw;
use pw::spa;
use pw::spa::param::video::{VideoFormat, VideoInfoRaw};
use pw::stream::StreamState;
use tracing::{debug, info};

use crate::pixels::{ChannelOrder, pack_bgra};

const MAX_FPS: u32 = 60;

#[derive(Default)]
struct State {
    /// Latest frame not yet taken: a slow consumer skips frames instead of lagging behind.
    frame: Option<VideoFrame>,
    ended: Option<String>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

impl Shared {
    fn push(&self, frame: VideoFrame) {
        self.state.lock().unwrap().frame = Some(frame);
        self.wake.notify_one();
    }

    /// The first reason wins: it is the cause, later ones are consequences.
    fn end(&self, reason: impl Into<String>) {
        self.state.lock().unwrap().ended.get_or_insert_with(|| reason.into());
        self.wake.notify_one();
    }
}

struct Quit;

pub struct PipewireStream {
    shared: Arc<Shared>,
    quit: pw::channel::Sender<Quit>,
    thread: Option<JoinHandle<()>>,
}

impl PipewireStream {
    /// `remote` is the PipeWire connection handed out by the portal; `None` connects to the
    /// user's default daemon.
    pub fn connect(remote: Option<OwnedFd>, node_id: u32) -> anyhow::Result<Self> {
        let shared = Arc::new(Shared::default());
        let (quit, quit_receiver) = pw::channel::channel();
        let thread = std::thread::Builder::new().name("pipewire".into()).spawn({
            let shared = shared.clone();
            move || match run(remote, node_id, &shared, quit_receiver) {
                Ok(()) => shared.end("capture stopped"),
                Err(e) => shared.end(format!("{e:#}")),
            }
        })?;
        Ok(Self { shared, quit, thread: Some(thread) })
    }

    /// Waits at most `timeout` for a frame. `Ok(None)`: nothing new. `Err`: the stream is over.
    pub fn next_frame(&self, timeout: Duration) -> anyhow::Result<Option<VideoFrame>> {
        let mut state = self.shared.state.lock().unwrap();
        if state.frame.is_none() && state.ended.is_none() {
            state = self.shared.wake.wait_timeout(state, timeout).unwrap().0;
        }
        if let Some(frame) = state.frame.take() {
            return Ok(Some(frame));
        }
        match &state.ended {
            Some(reason) => Err(anyhow!("{reason}")),
            None => Ok(None),
        }
    }

    /// Lets another thread end the stream with a reason. Needed because PipeWire itself only
    /// pauses, without any error, when the node it reads from disappears.
    pub(super) fn ender(&self) -> impl Fn(&str) + Send + 'static {
        let shared = self.shared.clone();
        move |reason| shared.end(reason)
    }

    pub fn stop(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = self.quit.send(Quit);
            let _ = thread.join();
        }
    }
}

impl Drop for PipewireStream {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run(
    remote: Option<OwnedFd>,
    node_id: u32,
    shared: &Arc<Shared>,
    quit_receiver: pw::channel::Receiver<Quit>,
) -> anyhow::Result<()> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = match remote {
        Some(fd) => context.connect_fd_rc(fd, None)?,
        None => context.connect_rc(None)?,
    };
    let _quit = quit_receiver.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        move |_| mainloop.quit()
    });
    let _core_listener = core
        .add_listener_local()
        .error({
            let shared = shared.clone();
            let mainloop = mainloop.clone();
            move |id, _seq, res, message| {
                debug!(id, res, message, "pipewire error");
                // Object 0 is the connection itself (daemon restarted, portal closed the remote).
                if id == pw::core::PW_ID_CORE {
                    shared.end(format!("lost the connection to PipeWire: {message}"));
                    mainloop.quit();
                }
            }
        })
        .register();

    let stream = pw::stream::StreamBox::new(
        &core,
        "miroir-capture",
        pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )?;

    let started = Instant::now();
    let _listener = stream
        .add_local_listener_with_user_data(VideoInfoRaw::default())
        .state_changed({
            let shared = shared.clone();
            let mainloop = mainloop.clone();
            move |_, _, old, new| {
                debug!(?old, ?new, "pipewire stream state");
                let was_running = matches!(old, StreamState::Paused | StreamState::Streaming);
                match new {
                    StreamState::Error(message) => {
                        shared.end(message);
                        mainloop.quit();
                    }
                    StreamState::Unconnected if was_running => {
                        shared.end("the captured screen or window is gone");
                        mainloop.quit();
                    }
                    _ => {}
                }
            }
        })
        .param_changed(|_, format, id, param| {
            let Some(param) = param else { return };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            if format.parse(param).is_ok() {
                let size = format.size();
                info!(
                    format = ?format.format(),
                    width = size.width,
                    height = size.height,
                    "capture format"
                );
            }
        })
        .process({
            let shared = shared.clone();
            move |stream, format| {
                let Some(mut buffer) = stream.dequeue_buffer() else { return };
                let Some(data) = buffer.datas_mut().first_mut() else { return };
                let order = match format.format() {
                    VideoFormat::BGRx | VideoFormat::BGRA => ChannelOrder::Bgr,
                    VideoFormat::RGBx | VideoFormat::RGBA => ChannelOrder::Rgb,
                    _ => return,
                };
                let chunk = data.chunk();
                let (offset, size) = (chunk.offset() as usize, chunk.size() as usize);
                let stride = chunk.stride().max(0) as usize;
                let Some(bytes) = data.data() else { return };
                let Some(bytes) = bytes.get(offset..offset.saturating_add(size)) else { return };

                let (width, height) = (format.size().width, format.size().height);
                match pack_bgra(bytes, stride, width, height, order) {
                    Some(bgra) => shared.push(VideoFrame {
                        width,
                        height,
                        pts_us: started.elapsed().as_micros() as u64,
                        data: FrameData::CpuBgra(bgra),
                    }),
                    // Empty chunks are normal (e.g. cursor-only updates): skip quietly.
                    None => debug!(size, stride, width, height, "skipped unusable buffer"),
                }
            }
        })
        .register()?;

    let format = serialize_pod(format_request())?;
    let mut params = [spa::pod::Pod::from_bytes(&format)
        .ok_or_else(|| anyhow!("invalid format request"))?];
    stream.connect(
        spa::utils::Direction::Input,
        Some(node_id),
        pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
        &mut params,
    )?;

    mainloop.run();
    Ok(())
}

/// What we accept from the source: 32-bit RGB in CPU memory, any size, at most `MAX_FPS`.
fn format_request() -> spa::pod::Object {
    use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
    use spa::pod::{object, property};
    use spa::utils::{Fraction, Rectangle};

    object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        property!(FormatProperties::MediaType, Id, MediaType::Video),
        property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        property!(
            FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRx,
            VideoFormat::BGRx,
            VideoFormat::BGRA,
            VideoFormat::RGBx,
            VideoFormat::RGBA,
        ),
        property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            Rectangle { width: 1920, height: 1080 },
            Rectangle { width: 1, height: 1 },
            Rectangle { width: 8192, height: 8192 }
        ),
        // 0/1 means "variable": frames come when the screen changes.
        property!(
            FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            Fraction { num: MAX_FPS, denom: 1 },
            Fraction { num: 0, denom: 1 },
            Fraction { num: MAX_FPS, denom: 1 }
        ),
        property!(
            FormatProperties::VideoMaxFramerate,
            Choice,
            Range,
            Fraction,
            Fraction { num: MAX_FPS, denom: 1 },
            Fraction { num: 1, denom: 1 },
            Fraction { num: MAX_FPS, denom: 1 }
        ),
    )
}

fn serialize_pod(object: spa::pod::Object) -> anyhow::Result<Vec<u8>> {
    let (cursor, _) = spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(object),
    )
    .map_err(|e| anyhow!("cannot serialize the format request: {e:?}"))?;
    Ok(cursor.into_inner())
}
