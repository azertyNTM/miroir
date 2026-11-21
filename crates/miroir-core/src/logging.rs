//! Logging setup shared by every binary.

use tracing_subscriber::EnvFilter;

/// Logs to stderr. The filter comes from `MIROIR_LOG` (e.g. `debug`, or
/// `info,miroir_net=trace`) and defaults to `info`.
pub fn init_logging() {
    tracing_subscriber::fmt()
        .with_env_filter(filter_from(std::env::var("MIROIR_LOG").ok().as_deref()))
        .with_writer(std::io::stderr)
        .init();
}

fn filter_from(value: Option<&str>) -> EnvFilter {
    // `info` stays the base level: EnvFilter reads an unknown word such as `debgu` as a
    // module name, which on its own would silence every log. Lossy parsing keeps the valid
    // directives and reports the invalid ones on stderr. Whitespace is dropped because
    // `set MIROIR_LOG=debug && app.exe` in cmd.exe leaves a trailing space in the value.
    let directives: String = value.unwrap_or("").split_whitespace().collect();
    EnvFilter::builder().parse_lossy(format!("info,{directives}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Buf {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().write(bytes)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// What actually gets written when an info and a debug event go through the filter.
    fn logged(value: Option<&str>) -> String {
        let buf = Buf::default();
        let writer = buf.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_env_filter(filter_from(value))
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("info-line");
            tracing::debug!("debug-line");
        });
        String::from_utf8(buf.0.lock().unwrap().clone()).unwrap()
    }

    #[test]
    fn filter_defaults_to_info() {
        let out = logged(None);
        assert!(out.contains("info-line") && !out.contains("debug-line"), "{out}");
    }

    #[test]
    fn filter_follows_the_variable() {
        let out = logged(Some("debug"));
        assert!(out.contains("info-line") && out.contains("debug-line"), "{out}");
    }

    #[test]
    fn warn_hides_info() {
        assert_eq!(logged(Some("warn")), "");
    }

    /// `set MIROIR_LOG=debug && miroir-app.exe` in cmd.exe keeps the space before `&&`.
    #[test]
    fn whitespace_is_ignored() {
        for value in [" debug ", "debug , miroir_net=trace"] {
            let out = logged(Some(value));
            assert!(out.contains("info-line") && out.contains("debug-line"), "{value:?}: {out}");
        }
    }

    #[test]
    fn one_bad_directive_keeps_the_valid_ones() {
        let out = logged(Some("debug,miroir_net=trcae"));
        assert!(out.contains("info-line") && out.contains("debug-line"), "{out}");
    }

    #[test]
    fn typo_does_not_silence_logs() {
        let out = logged(Some("debgu"));
        assert!(out.contains("info-line") && !out.contains("debug-line"), "{out}");
    }

    #[test]
    fn garbage_falls_back_to_info() {
        for value in ["pas valide!!", "miroir_core=notalevel", ""] {
            let out = logged(Some(value));
            assert!(out.contains("info-line") && !out.contains("debug-line"), "{value:?}: {out}");
        }
    }
}
