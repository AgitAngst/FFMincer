// В release-сборке прячем консольное окно (в debug оставляем для логов ffmpeg).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod ffmpeg;
mod sys;
mod texts;
mod ui;

fn main() -> eframe::Result<()> {
    // Следы прошлого обновления (*.old-…, папка загрузки) — прочь.
    anvil_update::cleanup();

    // eframe сам держит окно скрытым до первой отрисовки и показывает его при том размере,
    // что задан в билдере. Поэтому размер восстанавливаем ДО создания окна — иначе первый
    // видимый кадр был бы размера по умолчанию (прыжок/моргание). На первом запуске
    // геометрии ещё нет: окно подгонится и отцентрируется в App, а размер сохранится.
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_title("FFMincer")
        .with_min_inner_size([760.0, 480.0])
        .with_icon(std::sync::Arc::new(anvil_ui::appicon::icon_data(app::ACCENT, anvil_ui::Icon::Film)))
        .with_drag_and_drop(true);
    match config::load_geometry() {
        Some([w, h, x, y]) => {
            viewport = viewport.with_inner_size([w, h]).with_position([x, y]);
        }
        None => {
            // Близко к высоте, под которую подгоняются настройки по умолчанию, — чтобы даже
            // самый первый запуск (без конфига) не делал заметного ресайза.
            viewport = viewport.with_inner_size([1000.0, 960.0]);
        }
    }

    let native_options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native("FFMincer", native_options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}
