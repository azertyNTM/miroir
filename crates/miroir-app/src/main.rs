mod fps;
mod preview;

use std::path::PathBuf;

use anyhow::{Context, anyhow};
use eframe::egui;
use miroir_core::config::Config;
use tracing::{debug, info, warn};

use preview::{Event, Preview};

fn main() -> anyhow::Result<()> {
    miroir_core::logging::init_logging();
    info!("miroir-app v{}", env!("CARGO_PKG_VERSION"));

    let config_path = Config::default_path().context("no config directory on this system")?;
    debug!(path = %config_path.display(), "config path");
    let config = Config::load_from(&config_path)
        .with_context(|| format!("loading {}", config_path.display()))?;
    debug!(?config, "config loaded");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Miroir")
            .with_app_id("miroir")
            .with_inner_size([960.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Miroir",
        options,
        Box::new(move |_| Ok(Box::new(App { config, config_path, preview: None, texture: None, message: None }))),
    )
    .map_err(|e| anyhow!("cannot open the window: {e}"))
}

struct App {
    config: Config,
    config_path: PathBuf,
    preview: Option<Preview>,
    texture: Option<egui::TextureHandle>,
    /// Why the last capture stopped, when it was not on request.
    message: Option<String>,
}

impl App {
    fn start_preview(&mut self, ctx: &egui::Context) {
        let token = &self.config.capture_restore_token;
        let token = (!token.is_empty()).then(|| token.clone());
        self.message = None;
        self.preview = Some(Preview::start(ctx.clone(), token));
    }

    fn stop_preview(&mut self) {
        self.preview = None;
        self.texture = None;
    }

    /// Takes what the capture thread produced since the last repaint.
    fn poll_preview(&mut self, ctx: &egui::Context) {
        let Some(preview) = &self.preview else { return };

        if let Some(image) = preview.take_image() {
            let options = egui::TextureOptions::LINEAR;
            match &mut self.texture {
                Some(texture) => texture.set(image, options),
                None => self.texture = Some(ctx.load_texture("preview", image, options)),
            }
        }

        let events: Vec<Event> = std::iter::from_fn(|| preview.poll_event()).collect();
        for event in events {
            match event {
                Event::RestoreToken(token) => {
                    if token != self.config.capture_restore_token {
                        self.config.capture_restore_token = token;
                        if let Err(e) = self.config.save_to(&self.config_path) {
                            warn!("cannot save the config: {e}");
                        }
                    }
                }
                Event::Ended(reason) => {
                    self.stop_preview();
                    self.message = reason.map(|reason| format!("Capture arrêtée : {reason}"));
                }
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_preview(ui.ctx());

        egui::Panel::top("controls").show(ui, |ui| {
            ui.horizontal(|ui| {
                if self.preview.is_some() {
                    if ui.button("Arrêter").clicked() {
                        self.stop_preview();
                    }
                } else if ui.button("Choisir un écran ou une fenêtre").clicked() {
                    self.start_preview(ui.ctx());
                }

                match (&self.preview, &self.texture) {
                    (Some(preview), Some(texture)) => {
                        let [width, height] = texture.size();
                        let fps = preview.fps();
                        // A screen where nothing moves sends no frames: 0 fps is expected then.
                        let still = if fps == 0 { " (image fixe)" } else { "" };
                        ui.label(format!("{fps} fps — {width}×{height}{still}"));
                    }
                    (Some(_), None) => {
                        ui.label("En attente de la source…");
                    }
                    _ => {}
                }
                if let Some(message) = &self.message {
                    ui.colored_label(ui.visuals().warn_fg_color, message);
                }
            });
        });

        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(texture) = &self.texture {
                ui.centered_and_justified(|ui| {
                    ui.add(egui::Image::new(texture).fit_to_exact_size(ui.available_size()));
                });
            }
        });
    }
}
