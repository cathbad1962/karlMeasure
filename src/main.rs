mod pdf;
mod ui;

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
        Box::new(|_cc| Ok(Box::<ui::App>::default())),
    )
}
