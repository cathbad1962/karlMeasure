//! The application window: open a drawing, show page 1 fitted to the window.

use eframe::egui;

use crate::pdf;

/// Re-render only once the fitted size has moved by more than this many
/// pixels; the existing texture is scaled to fit in the meantime.
const RESIZE_TOLERANCE: u32 = 8;

#[derive(Default)]
pub struct App {
    document: Option<pdf::Document>,
    texture: Option<egui::TextureHandle>,
    rendered_for: Option<(u32, u32)>,
    error: Option<String>,
}

impl App {
    fn open_dialog(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("PDF drawing", &["pdf"])
            .pick_file()
        else {
            return;
        };

        self.texture = None;
        self.rendered_for = None;

        match pdf::Document::open(&path) {
            Ok(document) => {
                self.document = Some(document);
                self.error = None;
            }
            Err(message) => {
                self.document = None;
                self.error = Some(message);
            }
        }
    }

    /// Rasterises page 1 for `target` pixels if what we have is stale.
    fn refresh_texture(&mut self, ctx: &egui::Context, target: (u32, u32)) {
        if target.0 == 0 || target.1 == 0 {
            return;
        }

        let stale = match self.rendered_for {
            None => true,
            Some((width, height)) => {
                target.0.abs_diff(width) > RESIZE_TOLERANCE
                    || target.1.abs_diff(height) > RESIZE_TOLERANCE
            }
        };
        if !stale {
            return;
        }

        let rendered = match &self.document {
            Some(document) => document.render_first_page(target.0 as i32, target.1 as i32),
            None => return,
        };

        match rendered {
            Ok(raster) => {
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [raster.width, raster.height],
                    &raster.rgba,
                );
                self.texture = Some(ctx.load_texture("page", image, egui::TextureOptions::LINEAR));
                self.rendered_for = Some(target);
                self.error = None;
            }
            Err(message) => {
                self.document = None;
                self.error = Some(message);
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Open…").clicked() {
                    self.open_dialog();
                }
            });
        });

        egui::CentralPanel::default().show(ui, |ui| {
            if self.error.is_some() {
                let colour = ui.visuals().error_fg_color;
                let message = self.error.as_deref().unwrap_or_default();
                ui.colored_label(colour, message);
                return;
            }

            if self.document.is_none() {
                ui.centered_and_justified(|ui| ui.label("Open a PDF to begin."));
                return;
            }

            let available = ui.available_size();
            let ctx = ui.ctx().clone();
            let points_to_pixels = ctx.pixels_per_point();
            let target = (
                (available.x * points_to_pixels) as u32,
                (available.y * points_to_pixels) as u32,
            );

            self.refresh_texture(&ctx, target);

            if let Some(texture) = &self.texture {
                let size = texture.size_vec2();
                let fit = (available.x / size.x).min(available.y / size.y);
                ui.centered_and_justified(|ui| {
                    ui.add(egui::Image::new(texture).fit_to_exact_size(size * fit));
                });
            }
        });
    }
}
