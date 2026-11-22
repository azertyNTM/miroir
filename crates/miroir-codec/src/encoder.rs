//! H.264 encoder tuned for low latency. The first encoder that opens is used, hardware first.

use std::ptr;

use anyhow::{Context as _, bail, ensure};
use ffmpeg::ffi;
use ffmpeg::format::Pixel;
use ffmpeg_next as ffmpeg;
use miroir_core::frame::{FrameData, VideoFrame};
use tracing::{debug, info};

use crate::convert::{Scaler, bgra_to_nv12, check};
use crate::encoded_size;

const FPS: i32 = 60;
// ponytail: fixed bitrate. The setting (4 to 20 Mbit/s) and the adaptation to the network come
// with the transport (phases 5 and 7).
const BITRATE: usize = 10_000_000;
/// Long interval: a keyframe is big, and the viewer will ask for one when it needs it.
const KEYFRAME_INTERVAL_S: u32 = 4;

struct Candidate {
    name: &'static str,
    /// What the encoder is fed, in CPU memory.
    format: Pixel,
    /// GPU encoders that only take frames already in video memory: device kind and frame format.
    hardware: Option<(ffi::AVHWDeviceType, Pixel)>,
    options: &'static [(&'static str, &'static str)],
}

const VAAPI: Option<(ffi::AVHWDeviceType, Pixel)> =
    Some((ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI, Pixel::VAAPI));

/// In order of preference. Windows GPU encoders are added in phase 3.
const CANDIDATES: &[Candidate] = &[
    Candidate {
        name: "h264_vaapi",
        format: Pixel::NV12,
        hardware: VAAPI,
        // async_depth: without it, the first frame only comes back when the second goes in.
        // slices: lets the decoder work on several cores (`SLICES` in lib.rs).
        options: &[("async_depth", "1"), ("slices", "8")],
    },
    // Same, for the drivers that cannot cut a picture into slices.
    Candidate {
        name: "h264_vaapi",
        format: Pixel::NV12,
        hardware: VAAPI,
        options: &[("async_depth", "1")],
    },
    Candidate {
        name: "libx264",
        format: Pixel::NV12,
        hardware: None,
        options: &[("preset", "ultrafast"), ("tune", "zerolatency")],
    },
    Candidate {
        name: "libopenh264",
        format: Pixel::YUV420P,
        hardware: None,
        options: &[],
    },
];

/// One compressed picture, as an H.264 Annex B byte stream. Keyframes carry their own headers.
pub struct EncodedFrame {
    pub data: Vec<u8>,
    /// Timestamp of the captured frame it was made from.
    pub pts_us: u64,
    pub keyframe: bool,
}

pub struct Encoder {
    candidate: &'static Candidate,
    source_size: (u32, u32),
    encoder: ffmpeg::encoder::Video,
    /// Second step, when the encoder wants another size or format than our own conversion
    /// gives: swscale, and the NV12 picture at the source's size it starts from.
    scaler: Option<(Scaler, ffmpeg::frame::Video)>,
    /// What the encoder is fed.
    converted: ffmpeg::frame::Video,
    hw_frames: Option<BufferRef>,
}

impl Encoder {
    /// For a source of this size, in pixels. `forced` restricts the choice to one encoder name.
    pub fn new(width: u32, height: u32, forced: Option<&str>) -> anyhow::Result<Self> {
        match forced {
            None => Self::first_of(width, height, |_| true),
            Some(name) => {
                let known = || CANDIDATES.iter().map(|candidate| candidate.name);
                ensure!(
                    known().any(|k| k == name),
                    "unknown encoder {name}, not one of {:?}",
                    {
                        let mut names: Vec<_> = known().collect();
                        names.dedup();
                        names
                    }
                );
                Self::first_of(width, height, |candidate| candidate.name == name)
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn software(width: u32, height: u32) -> anyhow::Result<Self> {
        Self::first_of(width, height, |candidate| candidate.hardware.is_none())
    }

    fn first_of(
        width: u32,
        height: u32,
        wanted: impl Fn(&Candidate) -> bool,
    ) -> anyhow::Result<Self> {
        crate::init()?;
        ensure!(
            width >= 2 && height >= 2,
            "source too small ({width}x{height})"
        );
        let mut failures = Vec::new();
        for candidate in CANDIDATES.iter().filter(|candidate| wanted(candidate)) {
            match Self::open(candidate, width, height) {
                Ok(encoder) => {
                    let (w, h) = encoded_size(width, height);
                    info!(
                        encoder = candidate.name,
                        width = w,
                        height = h,
                        "encoder ready"
                    );
                    return Ok(encoder);
                }
                Err(e) => {
                    debug!(encoder = candidate.name, "not usable: {e:#}");
                    failures.push(format!("{}: {e:#}", candidate.name));
                }
            }
        }
        bail!("no usable H.264 encoder ({})", failures.join("; "))
    }

    fn open(candidate: &'static Candidate, width: u32, height: u32) -> anyhow::Result<Self> {
        let (out_width, out_height) = encoded_size(width, height);
        let codec = ffmpeg::encoder::find_by_name(candidate.name)
            .context("not available in this FFmpeg build")?;
        let mut video = ffmpeg::codec::Context::new_with_codec(codec)
            .encoder()
            .video()?;
        video.set_width(out_width);
        video.set_height(out_height);
        // Timestamps are the capture's microseconds: frames only come when the screen changes,
        // so they are not evenly spaced. The frame rate is what the rate control plans for.
        video.set_time_base((1, 1_000_000));
        video.set_frame_rate(Some((FPS, 1)));
        video.set_gop(FPS as u32 * KEYFRAME_INTERVAL_S);
        // B-frames need the next picture before they can be sent: one frame of delay each.
        video.set_max_b_frames(0);
        // Capped: a burst above the rate is what makes a stream stutter on a slow uplink.
        video.set_bit_rate(BITRATE);
        video.set_max_bit_rate(BITRATE);
        // SAFETY: `video` owns a valid, not yet opened AVCodecContext.
        unsafe { (*video.as_mut_ptr()).rc_buffer_size = (BITRATE / 4) as i32 };

        let hw_frames = match candidate.hardware {
            Some((device, hw_format)) => {
                let frames = hw_frames(device, hw_format, candidate.format, out_width, out_height)?;
                video.set_format(hw_format);
                // SAFETY: the context takes its own reference, released when it is freed.
                unsafe { (*video.as_mut_ptr()).hw_frames_ctx = ffi::av_buffer_ref(frames.0) };
                Some(frames)
            }
            None => {
                video.set_format(candidate.format);
                None
            }
        };
        let encoder = video.open_with(candidate.options.iter().copied().collect())?;

        // Our conversion gives NV12 at the source's size, less one pixel on an odd side.
        let from = (Pixel::NV12, width & !1, height & !1);
        let to = (candidate.format, out_width, out_height);
        let scaler = if from == to {
            None
        } else {
            Some((
                Scaler::new(from, to, true)?,
                ffmpeg::frame::Video::new(from.0, from.1, from.2),
            ))
        };

        Ok(Self {
            candidate,
            source_size: (width, height),
            encoder,
            scaler,
            converted: ffmpeg::frame::Video::new(candidate.format, out_width, out_height),
            hw_frames,
        })
    }

    /// FFmpeg's name for the encoder in use, e.g. `h264_vaapi`.
    pub fn name(&self) -> &'static str {
        self.candidate.name
    }

    /// True when the GPU does the work.
    pub fn is_hardware(&self) -> bool {
        self.candidate.hardware.is_some()
    }

    /// Size of the frames this encoder accepts. For another size, make another encoder.
    pub fn source_size(&self) -> (u32, u32) {
        self.source_size
    }

    /// `Ok(None)`: the encoder kept the frame and will give it back with a later one.
    pub fn encode(&mut self, frame: &VideoFrame) -> anyhow::Result<Option<EncodedFrame>> {
        let FrameData::CpuBgra(bgra) = &frame.data;
        let (width, height) = self.source_size;
        ensure!(
            (frame.width, frame.height) == (width, height),
            "the frame size changed"
        );
        let row = width as usize * 4;
        ensure!(
            bgra.len() == row * height as usize,
            "frame buffer does not match its size"
        );

        // A software encoder may still hold the previous picture: never write over it.
        // SAFETY: `converted` owns a valid frame with allocated buffers.
        check(unsafe { ffi::av_frame_make_writable(self.converted.as_mut_ptr()) })?;
        let nv12 = match &mut self.scaler {
            Some((_, nv12)) => nv12,
            None => &mut self.converted,
        };
        let size = (nv12.width() as usize, nv12.height() as usize);
        let (y_stride, uv_stride) = (nv12.stride(0), nv12.stride(1));
        // SAFETY: the two planes of an NV12 frame we own and may write to: separate memory
        // areas of `stride * rows` bytes, with half as many rows of colour.
        let (y, uv) = unsafe {
            let raw = nv12.as_mut_ptr();
            (
                std::slice::from_raw_parts_mut((*raw).data[0], y_stride * size.1),
                std::slice::from_raw_parts_mut((*raw).data[1], uv_stride * size.1 / 2),
            )
        };
        bgra_to_nv12(bgra, row, size, (y, y_stride), (uv, uv_stride));
        if let Some((scaler, nv12)) = &mut self.scaler {
            scaler.run(nv12, &mut self.converted)?;
        }
        let pts = Some(frame.pts_us as i64);
        self.converted.set_pts(pts);

        match &self.hw_frames {
            None => self.encoder.send_frame(&self.converted)?,
            Some(frames) => {
                let mut surface = ffmpeg::frame::Video::empty();
                // SAFETY: `frames` is an initialised frames context of the encoded size and
                // format, and both frames are valid for the duration of the calls.
                unsafe {
                    check(ffi::av_hwframe_get_buffer(
                        frames.0,
                        surface.as_mut_ptr(),
                        0,
                    ))?;
                    check(ffi::av_hwframe_transfer_data(
                        surface.as_mut_ptr(),
                        self.converted.as_ptr(),
                        0,
                    ))?;
                }
                surface.set_pts(pts);
                self.encoder.send_frame(&surface)?;
            }
        }

        let mut packet = ffmpeg::Packet::empty();
        match self.encoder.receive_packet(&mut packet) {
            Ok(()) => Ok(Some(EncodedFrame {
                data: packet.data().unwrap_or_default().to_vec(),
                pts_us: packet.pts().unwrap_or_default() as u64,
                keyframe: packet.is_key(),
            })),
            Err(ffmpeg::Error::Other {
                errno: ffmpeg::error::EAGAIN,
            }) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

/// Owns one reference to an FFmpeg reference-counted buffer.
struct BufferRef(*mut ffi::AVBufferRef);

impl Drop for BufferRef {
    fn drop(&mut self) {
        // SAFETY: the pointer is a reference we own, or null, which FFmpeg accepts.
        unsafe { ffi::av_buffer_unref(&mut self.0) };
    }
}

/// Opens the default device of this kind and prepares a pool of GPU frames on it.
fn hw_frames(
    device: ffi::AVHWDeviceType,
    hw_format: Pixel,
    sw_format: Pixel,
    width: u32,
    height: u32,
) -> anyhow::Result<BufferRef> {
    // SAFETY: plain FFmpeg hardware-context setup. Every pointer is checked before use, and
    // each reference is owned by a `BufferRef` as soon as it exists.
    unsafe {
        let mut device_ref = ptr::null_mut();
        let created =
            ffi::av_hwdevice_ctx_create(&mut device_ref, device, ptr::null(), ptr::null_mut(), 0);
        let device_ref = BufferRef(device_ref);
        check(created).context("no usable graphics device")?;

        let frames = BufferRef(ffi::av_hwframe_ctx_alloc(device_ref.0));
        ensure!(!frames.0.is_null(), "out of memory");
        let context = (*frames.0).data as *mut ffi::AVHWFramesContext;
        (*context).format = hw_format.into();
        (*context).sw_format = sw_format.into();
        (*context).width = width as i32;
        (*context).height = height as i32;
        check(ffi::av_hwframe_ctx_init(frames.0)).context("cannot allocate frames on the GPU")?;
        Ok(frames)
    }
}
