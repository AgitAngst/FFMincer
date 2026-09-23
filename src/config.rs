// Сохраняемые настройки приложения: файл `%APPDATA%\FFMincer\config.txt`, строки `ключ=значение`.
// Формат прежний — старый файл читается как есть; общие настройки семьи Anvil (тема, язык,
// обновления) живут в тех же строках.

use std::path::PathBuf;

use anvil_ui::{CommonSettings, Lang, ThemeChoice};

#[derive(Clone, Copy, PartialEq)]
pub enum PostAction {
    None,
    OpenFolder,
    Sleep,
    Shutdown,
}

#[derive(Clone)]
pub struct AppConfig {
    /// Тема, язык, проверка обновлений — как во всех программах семьи.
    pub common: CommonSettings,
    pub geometry: Option<[f32; 4]>,
    pub autostart_queue: bool,
    pub autoclear_finished: bool,
    pub sound_on_finish: bool,
    pub post_action: PostAction,
    pub ffmpeg_path: String,
    pub ffprobe_path: String,
    pub low_priority: bool,
    pub threads: u32,
    pub output_dir: String, // "" = рядом с исходником
    pub name_template: String,
    pub overwrite: bool, // true = перезаписывать, false = переименовывать
    pub keep_metadata: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            common: CommonSettings::default(),
            geometry: None,
            autostart_queue: false,
            autoclear_finished: false,
            sound_on_finish: false,
            post_action: PostAction::None,
            ffmpeg_path: String::new(),
            ffprobe_path: String::new(),
            low_priority: false,
            threads: 0,
            output_dir: String::new(),
            name_template: "{name}".into(),
            overwrite: false,
            keep_metadata: false,
        }
    }
}

/// Путь к файлу с сохранёнными настройками (`%APPDATA%\FFMincer\config.txt`).
fn config_path() -> Option<PathBuf> {
    let mut dir =
        std::env::var_os("APPDATA").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(PathBuf::from))?;
    dir.push("FFMincer");
    Some(dir.join("config.txt"))
}

pub fn load() -> AppConfig {
    let text = config_path().and_then(|path| std::fs::read_to_string(path).ok()).unwrap_or_default();
    parse(&text)
}

fn parse(text: &str) -> AppConfig {
    let mut c = AppConfig::default();
    let (mut w, mut h, mut x, mut y) = (None, None, None, None);
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        match key {
            "lang" => c.common.language = if value == "en" { Lang::En } else { Lang::Ru },
            "theme" => {
                c.common.theme = match value {
                    "light" => ThemeChoice::Light,
                    "dark" => ThemeChoice::Dark,
                    _ => ThemeChoice::System,
                }
            }
            "check_updates" => c.common.check_updates = value == "true",
            "prerelease" => c.common.prerelease = value == "true",
            "skip_version" => c.common.skip_version = (!value.is_empty()).then(|| value.to_string()),
            "win_w" => w = value.parse().ok(),
            "win_h" => h = value.parse().ok(),
            "win_x" => x = value.parse().ok(),
            "win_y" => y = value.parse().ok(),
            "autostart_queue" => c.autostart_queue = value == "true",
            "autoclear_finished" => c.autoclear_finished = value == "true",
            "sound_on_finish" => c.sound_on_finish = value == "true",
            "post_action" => {
                c.post_action = match value {
                    "open" => PostAction::OpenFolder,
                    "sleep" => PostAction::Sleep,
                    "shutdown" => PostAction::Shutdown,
                    _ => PostAction::None,
                }
            }
            "ffmpeg_path" => c.ffmpeg_path = value.to_string(),
            "ffprobe_path" => c.ffprobe_path = value.to_string(),
            "low_priority" => c.low_priority = value == "true",
            "threads" => c.threads = value.parse().unwrap_or(0),
            "output_dir" => c.output_dir = value.to_string(),
            "name_template" => c.name_template = value.to_string(),
            "overwrite" => c.overwrite = value == "true",
            "keep_metadata" => c.keep_metadata = value == "true",
            _ => {}
        }
    }
    if c.name_template.trim().is_empty() {
        c.name_template = "{name}".into();
    }
    c.geometry = match (w, h, x, y) {
        (Some(w), Some(h), Some(x), Some(y)) => Some([w, h, x, y]),
        _ => None,
    };
    c
}

/// Геометрия окна для билдера в `main.rs` (чтобы окно сразу открывалось нужного размера).
pub fn load_geometry() -> Option<[f32; 4]> {
    load().geometry
}

pub fn save(c: &AppConfig) {
    let Some(path) = config_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, render(c));
}

fn render(c: &AppConfig) -> String {
    let lang = match c.common.language {
        Lang::Ru => "ru",
        Lang::En => "en",
    };
    let theme = match c.common.theme {
        ThemeChoice::Dark => "dark",
        ThemeChoice::Light => "light",
        ThemeChoice::System => "system",
    };
    let post = match c.post_action {
        PostAction::None => "none",
        PostAction::OpenFolder => "open",
        PostAction::Sleep => "sleep",
        PostAction::Shutdown => "shutdown",
    };
    let mut text = String::new();
    text.push_str(&format!("lang={lang}\n"));
    text.push_str(&format!("theme={theme}\n"));
    text.push_str(&format!("check_updates={}\n", c.common.check_updates));
    text.push_str(&format!("prerelease={}\n", c.common.prerelease));
    text.push_str(&format!("skip_version={}\n", c.common.skip_version.as_deref().unwrap_or("")));
    text.push_str(&format!("autostart_queue={}\n", c.autostart_queue));
    text.push_str(&format!("autoclear_finished={}\n", c.autoclear_finished));
    text.push_str(&format!("sound_on_finish={}\n", c.sound_on_finish));
    text.push_str(&format!("post_action={post}\n"));
    text.push_str(&format!("ffmpeg_path={}\n", c.ffmpeg_path));
    text.push_str(&format!("ffprobe_path={}\n", c.ffprobe_path));
    text.push_str(&format!("low_priority={}\n", c.low_priority));
    text.push_str(&format!("threads={}\n", c.threads));
    text.push_str(&format!("output_dir={}\n", c.output_dir));
    text.push_str(&format!("name_template={}\n", c.name_template));
    text.push_str(&format!("overwrite={}\n", c.overwrite));
    text.push_str(&format!("keep_metadata={}\n", c.keep_metadata));
    if let Some([w, h, x, y]) = c.geometry {
        text.push_str(&format!("win_w={w:.0}\nwin_h={h:.0}\nwin_x={x:.0}\nwin_y={y:.0}\n"));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Файл прежней версии (без строк обновлений) читается, тема и язык переезжают в общие настройки.
    #[test]
    fn old_file_still_reads() {
        let c = parse("lang=en\ntheme=dark\npost_action=open\nthreads=4\nwin_w=900\nwin_h=700\nwin_x=10\nwin_y=20\n");
        assert_eq!(c.common.language, Lang::En);
        assert_eq!(c.common.theme, ThemeChoice::Dark);
        assert!(c.common.check_updates);
        assert!(c.post_action == PostAction::OpenFolder);
        assert_eq!(c.threads, 4);
        assert_eq!(c.geometry, Some([900.0, 700.0, 10.0, 20.0]));
    }

    #[test]
    fn round_trip() {
        let mut c = AppConfig::default();
        c.common.theme = ThemeChoice::Light;
        c.common.language = Lang::Ru;
        c.common.check_updates = false;
        c.common.skip_version = Some("0.2.0".into());
        c.name_template = "{name}_small".into();
        let back = parse(&render(&c));
        assert_eq!(back.common, c.common);
        assert_eq!(back.name_template, c.name_template);
    }
}
