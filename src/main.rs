// В release-сборке прячем консольное окно (в debug оставляем для логов ffmpeg).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod ffmpeg;
mod sys;

fn main() -> eframe::Result<()> {
    // eframe сам держит окно скрытым до первой отрисовки и показывает его при том размере,
    // что задан в билдере. Поэтому размер восстанавливаем ДО создания окна — иначе первый
    // видимый кадр был бы размера по умолчанию (прыжок/моргание). На первом запуске
    // геометрии ещё нет: окно подгонится и отцентрируется в App, а размер сохранится.
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_min_inner_size([560.0, 400.0])
        .with_drag_and_drop(true);
    match app::load_geometry() {
        Some([w, h, x, y]) => {
            viewport = viewport
                .with_inner_size([w, h])
                .with_position([x, y]);
        }
        None => {
            // Близко к высоте, под которую подгоняются дефолтные настройки, — чтобы даже
            // самый первый запуск (без конфига) не делал заметного ресайза.
            viewport = viewport.with_inner_size([900.0, 960.0]);
        }
    }

    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "FFMincer",
        native_options,
        // Примечание: если ваша версия eframe старше 0.27, замените строку ниже на
        // `Box::new(|cc| Box::new(app::App::new(cc)))` (без Ok(...)).
        Box::new(|cc| Box::new(app::App::new(cc))),
    )
}
