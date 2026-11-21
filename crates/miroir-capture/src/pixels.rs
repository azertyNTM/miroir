//! Turns a captured buffer into the tightly packed BGRA layout of `FrameData::CpuBgra`.

/// Byte order of the colour channels in the source buffer (the 4th byte is alpha or padding).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ChannelOrder {
    Bgr,
    Rgb,
}

/// Copies `height` rows of `width` pixels out of `src`, whose rows are `stride` bytes apart.
/// Returns `None` instead of panicking when the buffer does not match the announced geometry.
pub fn pack_bgra(
    src: &[u8],
    stride: usize,
    width: u32,
    height: u32,
    order: ChannelOrder,
) -> Option<Vec<u8>> {
    let row = (width as usize).checked_mul(4)?;
    let rows = height as usize;
    if row == 0 || rows == 0 || stride < row {
        return None;
    }
    // The last row may end right after its pixels, without padding.
    let needed = stride.checked_mul(rows - 1)?.checked_add(row)?;
    if src.len() < needed {
        return None;
    }

    let mut out = Vec::with_capacity(row * rows);
    for line in src.chunks(stride).take(rows) {
        out.extend_from_slice(&line[..row]);
    }
    if order == ChannelOrder::Rgb {
        for pixel in out.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tight_buffer_is_copied_as_is() {
        let src: Vec<u8> = (0..16).collect(); // 2x2
        assert_eq!(pack_bgra(&src, 8, 2, 2, ChannelOrder::Bgr).unwrap(), src);
    }

    #[test]
    fn row_padding_is_removed() {
        // 1x2 with a 6-byte stride: 2 padding bytes after each pixel.
        let src = [1, 2, 3, 4, 99, 99, 5, 6, 7, 8, 99, 99];
        assert_eq!(pack_bgra(&src, 6, 1, 2, ChannelOrder::Bgr).unwrap(), [1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn last_row_may_stop_before_the_padding() {
        let src = [1, 2, 3, 4, 99, 99, 5, 6, 7, 8];
        assert_eq!(pack_bgra(&src, 6, 1, 2, ChannelOrder::Bgr).unwrap(), [1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn rgb_order_is_swapped_to_bgr() {
        let src = [10, 20, 30, 40, 99, 99]; // R, G, B, A + padding
        assert_eq!(pack_bgra(&src, 6, 1, 1, ChannelOrder::Rgb).unwrap(), [30, 20, 10, 40]);
    }

    #[test]
    fn mismatched_geometry_is_rejected() {
        let src = [0u8; 16];
        assert_eq!(pack_bgra(&src, 8, 2, 3, ChannelOrder::Bgr), None, "buffer too short");
        assert_eq!(pack_bgra(&src, 4, 2, 2, ChannelOrder::Bgr), None, "stride below row size");
        assert_eq!(pack_bgra(&src, 8, 0, 2, ChannelOrder::Bgr), None, "empty frame");
        assert_eq!(pack_bgra(&src, 8, u32::MAX, u32::MAX, ChannelOrder::Bgr), None, "overflow");
    }
}
