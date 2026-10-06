//! Значок: `assets/icon/ffmincer.ico` — ресурс exe; PNG 256 px — в сырые пиксели RGBA для значка окна
//! (`src/icon.rs`). Файлы собирает `scripts/render-icon.ps1` из SVG.

use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR")).join("assets/icon");
    let (ico, png) = (dir.join("ffmincer.ico"), dir.join("ffmincer-256.png"));
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", ico.display());
    println!("cargo:rerun-if-changed={}", png.display());

    // Значок окна нужен на любой ОС: пиксели кладём в OUT_DIR, без декодера PNG в самой программе.
    let image = image::open(&png).expect("read assets/icon/ffmincer-256.png").into_rgba8();
    assert_eq!((image.width(), image.height()), (256, 256), "the window icon must be 256 x 256");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    std::fs::write(out.join("window.rgba"), image.as_raw()).expect("write window.rgba");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon(ico.to_str().expect("utf-8 path"));
    res.set("ProductName", "FFMincer");
    res.set("FileDescription", "FFMincer — audio and video converter");
    if let Err(e) = res.compile() {
        // Без rc.exe программа соберётся, просто без значка у exe.
        println!("cargo:warning=exe icon skipped: {e}");
    }
}
