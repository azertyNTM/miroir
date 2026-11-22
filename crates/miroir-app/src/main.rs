mod preview;
mod stats;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, anyhow};
use eframe::egui;
use miroir_core::config::Config;
use tracing::{debug, info, warn};

use preview::{Event, Preview};
use stats::Summary;

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
        Box::new(move |creation| {
            let mut app = App {
                config,
                config_path,
                preview: None,
                texture: None,
                message: None,
            };
            // Diagnostics (docs/test-manuel.md): no picker to click through, start right away.
            if std::env::var_os("MIROIR_PW_NODE").is_some() {
                app.start_preview(&creation.egui_ctx, false);
            }
            Ok(Box::new(app))
        }),
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
    /// `reuse_last_source`: ask the desktop for the source picked last time, without its picker.
    fn start_preview(&mut self, ctx: &egui::Context, reuse_last_source: bool) {
        let token = reuse_last_source.then(|| self.config.capture_restore_token.clone());
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
                if let Some(preview) = &self.preview {
                    // Not while the desktop's picker is open: it cannot be closed from here.
                    let stop = ui.add_enabled(preview.is_open(), egui::Button::new("Arrêter"));
                    if stop.clicked() {
                        self.stop_preview();
                    }
                } else if self.config.capture_restore_token.is_empty() {
                    if ui.button("Choisir un écran ou une fenêtre").clicked() {
                        self.start_preview(ui.ctx(), false);
                    }
                } else {
                    if ui.button("Reprendre la dernière source").clicked() {
                        self.start_preview(ui.ctx(), true);
                    }
                    if ui.button("Choisir une autre source…").clicked() {
                        self.start_preview(ui.ctx(), false);
                    }
                }

                match &self.preview {
                    Some(preview) if !preview.is_open() => {
                        ui.label("Choisis la source dans la fenêtre du bureau…");
                    }
                    Some(_) if self.texture.is_none() => {
                        ui.label("En attente de la première image…");
                    }
                    _ => {}
                }
                if let Some(message) = &self.message {
                    ui.colored_label(ui.visuals().warn_fg_color, message);
                }
            });
            if let (Some(preview), Some(texture)) = (&self.preview, &self.texture) {
                ui.label(status_line(preview, texture.size()));
            }
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

/// What is being shown and what each frame costs, over the last second.
fn status_line(preview: &Preview, [width, height]: [usize; 2]) -> String {
    let summary = preview.summary();
    let mut line = format!("{} fps — {width}×{height}", summary.fps);
    if let Some((name, hardware)) = preview.encoder() {
        let kind = if hardware { "matériel" } else { "logiciel" };
        line += &format!(" — {name} ({kind})");
    }
    if summary.fps == 0 {
        // A screen where nothing moves sends no frames: 0 fps is expected then.
        return line + " — image fixe";
    }
    line + " — " + &timings(&summary)
}

fn timings(summary: &Summary) -> String {
    // The total is made from the two numbers as shown, so the line always adds up.
    let (encode, decode) = (tenths(summary.encode), tenths(summary.decode));
    format!(
        "encodage {} + décodage {} = {} ms (pire image : {} ms) — affichage {} ms",
        ms(encode),
        ms(decode),
        ms(encode + decode),
        ms(tenths(summary.worst)),
        ms(tenths(summary.display)),
    )
}

/// Tenths of a millisecond, rounded to the nearest.
fn tenths(duration: Duration) -> u128 {
    (duration.as_micros() + 50) / 100
}

/// Tenths of a millisecond written the French way: 91 gives `9,1`.
fn ms(tenths: u128) -> String {
    format!("{},{}", tenths / 10, tenths % 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn milliseconds_use_a_decimal_comma() {
        assert_eq!(ms(tenths(Duration::from_micros(9_149))), "9,1");
        assert_eq!(ms(tenths(Duration::from_micros(9_150))), "9,2");
        assert_eq!(ms(tenths(Duration::ZERO)), "0,0");
        assert_eq!(ms(tenths(Duration::from_millis(12))), "12,0");
    }

    #[test]
    fn the_total_is_the_sum_of_the_two_numbers_shown() {
        let us = Duration::from_micros;
        // 7.04 + 1.54 = 8.58, which alone would round to 8,6.
        let summary = Summary {
            fps: 60,
            encode: us(7_040),
            decode: us(1_540),
            display: us(2_000),
            worst: us(10_260),
        };
        assert_eq!(
            timings(&summary),
            "encodage 7,0 + décodage 1,5 = 8,5 ms (pire image : 10,3 ms) — affichage 2,0 ms"
        );
    }
}
