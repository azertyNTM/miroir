//! Video frame exchanged between capture, codec and display.

pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    /// Capture timestamp in microseconds, from an arbitrary origin.
    pub pts_us: u64,
    pub data: FrameData,
}

pub enum FrameData {
    /// Tightly packed B, G, R, A bytes: `width * height * 4`, no row padding.
    CpuBgra(Vec<u8>),
}
