//! Video encoding and decoding through FFmpeg, with automatic encoder selection.

#[cfg(test)]
mod tests {
    use ffmpeg_next as ffmpeg;

    /// Proves the FFmpeg libraries are found at build time and loaded at run time.
    #[test]
    fn ffmpeg_links_and_has_h264_decoder() {
        ffmpeg::init().unwrap();
        assert!(ffmpeg::decoder::find(ffmpeg::codec::Id::H264).is_some());
    }
}
