//! Значок окна и панели задач — PNG 256 px из `assets/icon` (`scripts/render-icon.ps1`), которым `build.rs`
//! превращает в сырые пиксели. Значок exe — тоже `build.rs`; знак в шапке и «О программе» — `mark.rs`.

use eframe::egui;

const SIZE: u32 = 256;
/// RGBA без предумножения, 256 × 256 (кладёт `build.rs`).
const WINDOW: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/window.rgba"));

/// Значок для `ViewportBuilder::with_icon`; система сама уменьшает до нужного размера.
pub fn window() -> egui::IconData {
    egui::IconData { rgba: WINDOW.to_vec(), width: SIZE, height: SIZE }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_icon_is_a_256_square() {
        let icon = window();
        assert_eq!(icon.rgba.len(), (SIZE * SIZE * 4) as usize);
        // Плитка залита в центре, углы скруглены и прозрачны.
        assert_eq!(icon.rgba[((128 * SIZE + 128) * 4 + 3) as usize], 255);
        assert_eq!(icon.rgba[3], 0);
    }
}
