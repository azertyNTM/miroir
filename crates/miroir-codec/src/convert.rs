//! Colour conversion and resizing, on several cores.
//!
//! `bgra_to_nv12` is what every captured frame goes through. swscale would do the same after a
//! copy of the picture (it may read past the end of a row), in about 2 ms per 1080p frame with
//! threads on a Ryzen 5 2600X, where this takes about 1 ms. `Scaler` is swscale, for what
//! comes after when needed: a resize, another format.
//!
//! The matrix is BT.601 limited range (Y 16..235), the one swscale uses by default.

use std::ffi::CStr;
use std::ptr;

use anyhow::ensure;
use ffmpeg::ffi;
use ffmpeg::format::Pixel;
use ffmpeg::software::scaling;
use ffmpeg_next as ffmpeg;

/// How many pieces a picture is cut into, one per thread. Measured in 1080p: 3.2 ms with 1,
/// 0.95 ms with 4, 0.8 ms with 6. The total work is the same, it is only done sooner.
const BANDS: usize = 6;

/// A swscale context that can use several threads, which ffmpeg-next's own wrapper cannot ask
/// for.
pub(crate) struct Scaler {
    context: *mut ffi::SwsContext,
    from: (Pixel, u32, u32),
    to: (Pixel, u32, u32),
}

impl Scaler {
    /// `from` and `to`: pixel format, width, height.
    ///
    /// `threaded` must stay false when converting to RGB: swscale then writes whole blocks of
    /// pixels, past the end of a row and into the start of the next one. On one thread the
    /// next row is written afterwards and repairs it; with threads it may already be done,
    /// and a few wrong pixels stay at the left of the picture.
    pub(crate) fn new(
        from: (Pixel, u32, u32),
        to: (Pixel, u32, u32),
        threaded: bool,
    ) -> anyhow::Result<Self> {
        // SAFETY: the context is checked, then owned by `scaler`, which frees it. The options
        // are set by name because the struct's fields are only public in recent FFmpeg.
        unsafe {
            let context = ffi::sws_alloc_context();
            ensure!(!context.is_null(), "out of memory");
            let scaler = Self { context, from, to };
            let format = |pixel: Pixel| ffi::AVPixelFormat::from(pixel) as i64;
            // Bilinear: fast, and good enough when a 1440p or 4K source is reduced.
            let options: [(&CStr, i64); 8] = [
                (c"srcw", from.1.into()),
                (c"srch", from.2.into()),
                (c"src_format", format(from.0)),
                (c"dstw", to.1.into()),
                (c"dsth", to.2.into()),
                (c"dst_format", format(to.0)),
                (c"sws_flags", scaling::Flags::BILINEAR.bits().into()),
                (c"threads", if threaded { BANDS as i64 } else { 1 }),
            ];
            for (name, value) in options {
                check(ffi::av_opt_set_int(context.cast(), name.as_ptr(), value, 0))?;
            }
            check(ffi::sws_init_context(
                context,
                ptr::null_mut(),
                ptr::null_mut(),
            ))?;
            Ok(scaler)
        }
    }

    /// What `run` expects as its source: pixel format, width, height.
    pub(crate) fn from(&self) -> (Pixel, u32, u32) {
        self.from
    }

    /// Both frames must have the formats and sizes given to `new`, and allocated buffers.
    pub(crate) fn run(
        &mut self,
        from: &ffmpeg::frame::Video,
        to: &mut ffmpeg::frame::Video,
    ) -> anyhow::Result<()> {
        let shape = |frame: &ffmpeg::frame::Video| (frame.format(), frame.width(), frame.height());
        // swscale does not check the destination: too small, it would write outside of it.
        ensure!(
            shape(from) == self.from && shape(to) == self.to,
            "unexpected picture"
        );
        // SAFETY: a valid, initialised context, and two valid frames of the formats and sizes
        // it was initialised for, checked just above.
        check(unsafe { ffi::sws_scale_frame(self.context, to.as_mut_ptr(), from.as_ptr()) })?;
        Ok(())
    }
}

impl Drop for Scaler {
    fn drop(&mut self) {
        // SAFETY: the context we allocated; nothing else holds it.
        unsafe { ffi::sws_freeContext(self.context) };
    }
}

pub(crate) fn check(code: i32) -> Result<(), ffmpeg::Error> {
    if code < 0 {
        Err(ffmpeg::Error::from(code))
    } else {
        Ok(())
    }
}

fn luma(b: u8, g: u8, r: u8) -> u8 {
    ((66 * r as u32 + 129 * g as u32 + 25 * b as u32 + 128) >> 8) as u8 + 16
}

/// Fills an NV12 picture of `width` x `height` (both even) from the top-left corner of a BGRA
/// picture whose rows are `bgra_stride` bytes apart. Panics if a buffer is too small.
pub(crate) fn bgra_to_nv12(
    bgra: &[u8],
    bgra_stride: usize,
    (width, height): (usize, usize),
    (y, y_stride): (&mut [u8], usize),
    (uv, uv_stride): (&mut [u8], usize),
) {
    assert!(width.is_multiple_of(2) && height.is_multiple_of(2) && width * 4 <= bgra_stride);
    assert!(bgra.len() >= bgra_stride * height && y_stride >= width && uv_stride >= width);
    assert!(y.len() >= y_stride * height && uv.len() >= uv_stride * height / 2);
    // Rows in each band, always an even number: a row of colour samples covers two rows.
    let rows = (height / 2).div_ceil(BANDS).max(1) * 2;
    std::thread::scope(|scope| {
        let bands = bgra[..bgra_stride * height]
            .chunks(bgra_stride * rows)
            .zip(y.chunks_mut(y_stride * rows))
            .zip(uv.chunks_mut(uv_stride * rows / 2));
        for ((bgra, y), uv) in bands {
            scope.spawn(move || {
                let pairs = bgra
                    .chunks(bgra_stride * 2)
                    .zip(y.chunks_mut(y_stride * 2))
                    .zip(uv.chunks_mut(uv_stride));
                for ((bgra, y), uv) in pairs {
                    let (top, bottom) = bgra.split_at(bgra_stride);
                    let (top, bottom) = (&top[..width * 4], &bottom[..width * 4]);
                    let (y_top, y_bottom) = y.split_at_mut(y_stride);
                    for (y, px) in y_top[..width].iter_mut().zip(top.chunks_exact(4)) {
                        *y = luma(px[0], px[1], px[2]);
                    }
                    for (y, px) in y_bottom[..width].iter_mut().zip(bottom.chunks_exact(4)) {
                        *y = luma(px[0], px[1], px[2]);
                    }
                    let quads = top.chunks_exact(8).zip(bottom.chunks_exact(8));
                    for (uv, (top, bottom)) in uv[..width].chunks_exact_mut(2).zip(quads) {
                        // One colour sample for the four pixels: their average.
                        let sum = |i: usize| {
                            top[i] as i32
                                + top[i + 4] as i32
                                + bottom[i] as i32
                                + bottom[i + 4] as i32
                        };
                        let (b, g, r) = (sum(0), sum(1), sum(2));
                        uv[0] = (((-38 * r - 74 * g + 112 * b + 512) >> 10) + 128) as u8;
                        uv[1] = (((112 * r - 94 * g - 18 * b + 512) >> 10) + 128) as u8;
                    }
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Converts the top-left corner of a tightly packed BGRA picture to tightly packed planes.
    fn to_nv12(bgra: &[u8], src_width: usize, width: usize, height: usize) -> (Vec<u8>, Vec<u8>) {
        let (mut y, mut uv) = (vec![0; width * height], vec![0; width * height / 2]);
        bgra_to_nv12(
            bgra,
            src_width * 4,
            (width, height),
            (&mut y, width),
            (&mut uv, width),
        );
        (y, uv)
    }

    #[test]
    fn scaler_refuses_frames_it_was_not_made_for() {
        let frame = |side| ffmpeg::frame::Video::new(Pixel::NV12, side, side);
        let mut scaler = Scaler::new((Pixel::NV12, 64, 64), (Pixel::NV12, 32, 32), true).unwrap();
        assert!(scaler.run(&frame(64), &mut frame(32)).is_ok());
        assert!(
            scaler.run(&frame(64), &mut frame(16)).is_err(),
            "destination too small"
        );
        assert!(
            scaler.run(&frame(32), &mut frame(32)).is_err(),
            "other source"
        );
    }

    #[test]
    fn known_colours_get_their_standard_values() {
        for (bgra, yuv) in [
            ([255, 255, 255, 255], [235, 128, 128]), // white
            ([0, 0, 0, 255], [16, 128, 128]),        // black
            ([0, 0, 255, 255], [82, 90, 240]),       // red
            ([0, 255, 0, 255], [144, 54, 34]),       // green (145 in floating point)
            ([255, 0, 0, 255], [41, 240, 110]),      // blue
        ] {
            let (y, uv) = to_nv12(&bgra.repeat(4 * 2), 4, 4, 2);
            assert_eq!(y, [yuv[0]; 8], "{bgra:?}");
            assert_eq!(uv, [yuv[1], yuv[2], yuv[1], yuv[2]], "{bgra:?}");
        }
    }

    #[test]
    fn colour_sample_is_the_average_of_its_four_pixels() {
        // Top row black then white, bottom row white then black: grey on average.
        let (black, white) = ([0, 0, 0, 255], [255; 4]);
        let bgra = [black, white, white, black].concat();
        let (y, uv) = to_nv12(&bgra, 2, 2, 2);
        assert_eq!(y, [16, 235, 235, 16]);
        assert_eq!(uv, [128, 128]);
    }

    /// Each row has its own grey: a band written at the wrong place, or twice, shows up.
    #[test]
    fn every_row_lands_where_it_belongs() {
        for height in [2, 6, 10, 64, 1080] {
            let grey = |row: usize| (row % 200) as u8;
            let bgra: Vec<u8> = (0..height).flat_map(|row| [grey(row); 16]).collect();
            let (y, uv) = to_nv12(&bgra, 4, 4, height);
            for (row, y) in y.chunks_exact(4).enumerate() {
                assert_eq!(
                    y,
                    [luma(grey(row), grey(row), grey(row)); 4],
                    "{height}, row {row}"
                );
            }
            assert!(uv.iter().all(|sample| *sample == 128), "grey has no colour");
        }
    }

    #[test]
    fn odd_source_is_cropped_and_row_padding_is_left_alone() {
        // 5x3 source, red except its last column and last row, which are white.
        let mut bgra = Vec::new();
        for row in 0..3 {
            for column in 0..5 {
                bgra.extend(if row == 2 || column == 4 {
                    [255; 4]
                } else {
                    [0, 0, 255, 255]
                });
            }
        }
        // Planes with 3 bytes of padding after each row, as FFmpeg frames have.
        let (mut y, mut uv) = (vec![7; 7 * 2], vec![7; 7]);
        bgra_to_nv12(&bgra, 5 * 4, (4, 2), (&mut y, 7), (&mut uv, 7));
        assert_eq!(y, [82, 82, 82, 82, 7, 7, 7, 82, 82, 82, 82, 7, 7, 7]);
        assert_eq!(uv, [90, 240, 90, 240, 7, 7, 7]);
    }
}
