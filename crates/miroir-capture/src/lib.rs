//! Screen and window capture (PipeWire portal on Linux, Windows Graphics Capture on Windows).

#[cfg(all(test, target_os = "linux"))]
mod tests {
    /// Proves libpipewire is found at build time and loaded at run time.
    #[test]
    fn pipewire_links() {
        pipewire::init();
    }
}
