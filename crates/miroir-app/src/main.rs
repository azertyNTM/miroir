use anyhow::Context;
use miroir_core::config::Config;
use tracing::{debug, info};

fn main() -> anyhow::Result<()> {
    miroir_core::logging::init_logging();
    info!("miroir-app v{}", env!("CARGO_PKG_VERSION"));

    let path = Config::default_path().context("no config directory on this system")?;
    debug!(path = %path.display(), "config path");
    let config = Config::load_from(&path).with_context(|| format!("loading {}", path.display()))?;
    debug!(?config, "config loaded");
    Ok(())
}
