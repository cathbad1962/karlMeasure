mod doc;
mod geom;
mod pdf;
mod tools;
mod ui;
mod viewport;

use eframe::egui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 800.0])
            .with_title("Measure"),
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
