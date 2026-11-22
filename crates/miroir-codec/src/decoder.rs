//! H.264 decoder that gives each picture back as soon as its data is in.

use anyhow::Context as _;
use ffmpeg::codec::threading;
use ffmpeg::format::Pixel;
use ffmpeg_next as ffmpeg;

use crate::EncodedFrame;
use crate::convert::Scaler;

/// A decoded picture, in the decoder's own format (YUV). See [`Decoder::to_rgba`].
pub struct DecodedFrame {
    pub width: u32,
    pub height: u32,
    pub pts_us: u64,
    picture: ffmpeg::frame::Video,
}

/// A picture ready for a screen texture.
pub struct RgbaFrame {
    pub width: u32,
    pub height: u32,
    picture: ffmpeg::frame::Video,
}

impl RgbaFrame {
    /// The picture's rows, top to bottom: `width * 4` bytes of R, G, B, A each. Alpha is 255.
    pub fn rows(&self) -> impl Iterator<Item = &[u8]> {
        let row = self.width as usize * 4;
        let lines = self.picture.data(0).chunks(self.picture.stride(0));
        lines
            .take(self.height as usize)
            .map(move |line| &line[..row])
    }
}

pub struct Decoder {
    decoder: ffmpeg::decoder::Video,
    /// For `to_rgba`. Remade when the stream changes size.
    scaler: Option<Scaler>,
}

impl Decoder {
    // ponytail: software decoding only. Measured on a RX 5500 XT in 1080p: the GPU decodes in
    // 1.5 ms but reading the picture back to CPU memory takes 3 ms more, against 2 to 4 ms
    // here. GPU decoding is worth it once the picture stays on the GPU (phase 9).
    pub fn new() -> anyhow::Result<Self> {
        crate::init()?;
        let codec = ffmpeg::decoder::find(ffmpeg::codec::Id::H264)
            .context("no H.264 decoder in this FFmpeg build")?;
        let mut context = ffmpeg::codec::Context::new_with_codec(codec);
        // Do not wait for later pictures before giving one back: our streams have no B-frames.
        context.set_flags(ffmpeg::codec::Flags::LOW_DELAY);
        // The slices of one picture in parallel. Unlike decoding several pictures at once
        // (FFmpeg's other threading mode), this adds no delay.
        // (`Config` has one more field with FFmpeg 5 and older, hence the `..`.)
        #[allow(clippy::needless_update)]
        context.set_threading(threading::Config {
            kind: threading::Type::Slice,
            count: crate::SLICES,
            ..Default::default()
        });
        Ok(Self {
            decoder: context.decoder().video()?,
            scaler: None,
        })
    }

    /// `Ok(None)`: the decoder needs more data before it can give a picture.
    pub fn decode(&mut self, frame: &EncodedFrame) -> anyhow::Result<Option<DecodedFrame>> {
        // A copy: FFmpeg wants zeroed padding after the data, which a `Vec` does not promise.
        let mut packet = ffmpeg::Packet::copy(&frame.data);
        packet.set_pts(Some(frame.pts_us as i64));
        self.decoder.send_packet(&packet)?;
        let mut picture = ffmpeg::frame::Video::empty();
        match self.decoder.receive_frame(&mut picture) {
            Ok(()) => Ok(Some(DecodedFrame {
                width: picture.width(),
                height: picture.height(),
                pts_us: picture.pts().unwrap_or_default() as u64,
                picture,
            })),
            Err(ffmpeg::Error::Other {
                errno: ffmpeg::error::EAGAIN,
            }) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    // ponytail: conversion on the CPU with swscale, for an egui texture.
    // The viewer's shader will take the decoder's YUV directly when that cost matters (phase 9).
    pub fn to_rgba(&mut self, frame: &DecodedFrame) -> anyhow::Result<RgbaFrame> {
        let (width, height) = (frame.width, frame.height);
        let from = (frame.picture.format(), width, height);
        let scaler = match &mut self.scaler {
            Some(scaler) if scaler.from() == from => scaler,
            slot => slot.insert(Scaler::new(from, (Pixel::RGBA, width, height), false)?),
        };
        let mut picture = ffmpeg::frame::Video::new(Pixel::RGBA, width, height);
        scaler.run(&frame.picture, &mut picture)?;
        Ok(RgbaFrame {
            width,
            height,
            picture,
        })
    }
}
