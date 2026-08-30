mod doc;
mod geom;
mod pdf;
mod tools;
mod ui;
mod viewport;

use eframe::egui;

/// The mark, drawn at build time and carried in the binary: see `build.rs`.
const ICON: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/icon.rgba"));

fn main() -> eframe::Result {
    let side = (ICON.len() as f64 / 4.0).sqrt() as u32;
    let icon = egui::IconData {
        rgba: ICON.to_vec(),
        width: side,
        height: side,
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 800.0])
            .with_title("Measure")
            .with_icon(icon),
        ..Default::default()
    };

    eframe::run_native(
        "Measure",
        options,
        Box::new(|cc| {
            // A tool strip of single letters is unreadable without its
            // tooltips, and half a second of holding still is long enough to
            // conclude there are none.
            cc.egui_ctx.all_styles_mut(|style| {
                style.interaction.tooltip_delay = 0.15;
                style.interaction.show_tooltips_only_when_still = false;
            });

            Ok(Box::<ui::App>::default())
        }),
    )
}
