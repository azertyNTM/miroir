use tracing::info;

fn main() {
    miroir_core::logging::init_logging();
    info!("miroir-server v{}", env!("CARGO_PKG_VERSION"));
}
