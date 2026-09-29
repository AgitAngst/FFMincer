//! Знак FFMincer в интерфейсе: две стрелки «вперёд» на розовой плитке — целая впереди и нарезанная позади
//! (след скорости и «фарш»). Форма — из `assets/icon/ffmincer-small.svg` (три ломтика вместо четырёх), цвета
//! от темы окна не зависят: это знак программы, а не деталь интерфейса. Набор рисует в «О программе» свой
//! значок из `Icon`, поэтому диалог тут собран заново из его частей.

use anvil_ui::chrome::{self, AboutAction, AppInfo};
use anvil_ui::widgets as w;
use anvil_ui::{Icon, Kind, Palette, semibold};
use eframe::egui::{self, Color32, Painter, Pos2, Rect, Response, RichText, Sense, Shape, Stroke, Ui, Vec2, pos2};

/// Плитка — середина её градиента `#F26A83 → #C4325A`.
const TILE: Color32 = Color32::from_rgb(0xDB, 0x4A, 0x6E);

/// Размеры в единицах SVG (плитка 1024×1024): ось стрелок, ширина и высота каждой, ломтики следа, сдвиг центровки.
const AXIS: f32 = 515.0;
const WIDTH: f32 = 300.0;
const HEIGHT: f32 = 504.0;
const LEAD_X: f32 = 512.0;
const PIECES: usize = 3;
const GAP: f32 = 28.0;
const DRIFT: f32 = 24.0;
const SHIFT: (f32, f32) = (6.0, -8.0);
/// Прозрачность ломтиков от дальнего к ближнему.
const FADE: [u8; PIECES] = [122, 173, 224];

/// Кусок правой стрелки (основание в `x0`) между вертикалями `xa` и `xb`: четыре угла.
fn slab(x0: f32, xa: f32, xb: f32) -> [(f32, f32); 4] {
    let top = |x: f32| AXIS - HEIGHT / 2.0 + (HEIGHT / 2.0) * (x - x0) / WIDTH;
    let bottom = |x: f32| AXIS + HEIGHT / 2.0 - (HEIGHT / 2.0) * (x - x0) / WIDTH;
    [(xa, top(xa)), (xb, top(xb)), (xb, bottom(xb)), (xa, bottom(xa))]
}

/// Знак в квадрате `rect`.
pub fn paint(painter: &Painter, rect: Rect) {
    let k = rect.width() / 1024.0;
    let at = |(x, y): (f32, f32)| -> Pos2 { pos2(rect.min.x + (x + SHIFT.0) * k, rect.min.y + (y + SHIFT.1) * k) };
    painter.rect_filled(rect, (rect.width() * 0.26).round() as u8, TILE);

    // Нарезанная стрелка позади: ломтики расходятся назад.
    let x1 = LEAD_X - WIDTH;
    let piece = (WIDTH - (PIECES as f32 - 1.0) * GAP) / PIECES as f32;
    for (i, alpha) in FADE.into_iter().enumerate() {
        let xa = x1 + i as f32 * (piece + GAP);
        let drift = -((PIECES - 1 - i) as f32) * DRIFT;
        let points = slab(x1, xa, xa + piece).iter().map(|&(x, y)| at((x + drift, y))).collect();
        painter.add(Shape::convex_polygon(points, Color32::from_white_alpha(alpha), Stroke::NONE));
    }
    // Целая стрелка впереди.
    let lead = vec![at((LEAD_X, AXIS - HEIGHT / 2.0)), at((LEAD_X, AXIS + HEIGHT / 2.0)), at((LEAD_X + WIDTH, AXIS))];
    painter.add(Shape::convex_polygon(lead, Color32::WHITE, Stroke::NONE));
}

/// Знак `size` на `size` в потоке интерфейса.
pub fn app_mark(ui: &mut Ui, size: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint(ui.painter(), rect);
    response
}

/// Знак и название в шапке (как `chrome::brand` набора, но со своим знаком).
pub fn brand(ui: &mut Ui, name: &str) {
    let p = Palette::of(ui);
    app_mark(ui, 28.0);
    ui.add_space(2.0);
    ui.label(RichText::new(name).font(semibold(16.0)).color(p.text));
}

/// «О программе» — как `chrome::about` набора, но со своим знаком. `status` — строка `anvil-update`.
pub fn about(ctx: &egui::Context, open: &mut bool, info: &AppInfo, status: Option<&str>) -> Option<AboutAction> {
    let mut action = None;
    chrome::dialog(ctx, "ffmincer-about", anvil_ui::tr(ctx, "О программе"), 400.0, open, |ui| {
        let p = Palette::of(ui);
        ui.vertical_centered(|ui| {
            ui.add_space(8.0);
            app_mark(ui, 56.0);
            ui.add_space(10.0);
            ui.label(RichText::new(info.name).font(semibold(20.0)).color(p.text));
            ui.label(
                RichText::new(format!("{} {}", anvil_ui::tr(ctx, "Версия"), info.version)).size(13.0).color(p.weak),
            );
            ui.add_space(6.0);
            ui.label(RichText::new(info.tagline).color(p.text));
            ui.add_space(12.0);
            if w::button(ui, Kind::Secondary, Some(Icon::Refresh), anvil_ui::tr(ctx, "Проверить обновления")).clicked()
            {
                action = Some(AboutAction::CheckUpdates);
            }
            if let Some(status) = status {
                ui.add_space(4.0);
                w::note(ui, status);
            }
            ui.add_space(10.0);
            if !info.repository.is_empty() {
                ui.hyperlink_to(RichText::new(anvil_ui::tr(ctx, "Исходный код")).size(13.0), info.repository);
            }
            ui.label(RichText::new(anvil_ui::tr(ctx, "Часть семьи Anvil")).size(12.0).color(p.faint));
        });
    });
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arrows_stay_inside_the_tile() {
        let left = LEAD_X - WIDTH - (PIECES as f32 - 1.0) * DRIFT + SHIFT.0;
        let right = LEAD_X + WIDTH + SHIFT.0;
        let (top, bottom) = (AXIS - HEIGHT / 2.0 + SHIFT.1, AXIS + HEIGHT / 2.0 + SHIFT.1);
        assert!(left > 0.0 && right < 1024.0 && top > 0.0 && bottom < 1024.0);
    }

    #[test]
    fn a_piece_is_a_slice_of_the_arrow() {
        // На основании кусок такой же высоты, что и стрелка; к острию он ниже.
        let base = slab(0.0, 0.0, 10.0);
        assert!((base[3].1 - base[0].1 - HEIGHT).abs() < 1e-3);
        let tip = slab(0.0, WIDTH - 10.0, WIDTH);
        assert!(tip[3].1 - tip[0].1 < HEIGHT / 10.0);
    }
}
