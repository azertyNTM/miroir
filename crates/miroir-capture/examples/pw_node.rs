//! Diagnostic tool: reads a PipeWire video node for 3 seconds and prints what arrives.
//!
//! See `docs/test-manuel.md` for how to publish a test pattern to read.

#[cfg(target_os = "linux")]
fn main() -> anyhow::Result<()> {
    use std::time::{Duration, Instant};

    use anyhow::Context;
    use miroir_capture::linux::PipewireStream;
    use miroir_core::frame::FrameData;

    miroir_core::logging::init_logging();
    let node_id = std::env::args().nth(1).context("usage: pw_node <node id>")?.parse()?;
    let stream = PipewireStream::connect(None, node_id)?;

    let started = Instant::now();
    let mut frames = 0;
    while started.elapsed() < Duration::from_secs(3) {
        if let Some(frame) = stream.next_frame(Duration::from_millis(100))? {
            let FrameData::CpuBgra(bgra) = &frame.data;
            if frames == 0 {
                println!("first frame: {}x{}, first pixel BGRA {:?}", frame.width, frame.height, &bgra[..4]);
            }
            assert_eq!(bgra.len(), (frame.width * frame.height * 4) as usize);
            frames += 1;
        }
    }
    println!("{frames} frames in 3 s = {} fps", frames / 3);
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn main() {}
