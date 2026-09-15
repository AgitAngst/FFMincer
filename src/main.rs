mod app;
mod ffmpeg;

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([880.0, 620.0])
            .with_min_inner_size([640.0, 420.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };

    eframe::run_native(
        "FFMincer",
        native_options,
        // Примечание: если ваша версия eframe старше 0.27, замените строку ниже на
        // `Box::new(|cc| Box::new(app::App::new(cc)))` (без Ok(...)).
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
