// Логика работы с ffmpeg: настройки job'а, сборка аргументов, выполнение очереди
// в фоновом потоке с отправкой событий прогресса в UI через mpsc::channel.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

/// Не создавать консольное окно для дочернего процесса (иначе на Windows
/// каждый вызов ffmpeg/ffprobe мигал бы чёрным окном при скрытой консоли).
#[cfg(windows)]
fn hide_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_window(_cmd: &mut Command) {}

/// Как `hide_window`, но с возможностью понизить приоритет процесса (для ffmpeg).
#[cfg(windows)]
fn hide_window_prio(cmd: &mut Command, low_priority: bool) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
    let mut flags = CREATE_NO_WINDOW;
    if low_priority {
        flags |= BELOW_NORMAL_PRIORITY_CLASS;
    }
    cmd.creation_flags(flags);
}

#[cfg(not(windows))]
fn hide_window_prio(_cmd: &mut Command, _low_priority: bool) {}

#[derive(Debug, Clone)]
pub struct JobSettings {
    pub video_codec: String,
    pub audio_codec: String,
    pub crf: u32,
    pub video_bitrate: String, // пусто = не задан, используется crf (для CPU-кодеков)
    pub audio_bitrate: String, // пусто = стандартный битрейт кодека
    pub preset: String,        // пусто = не передавать -preset
    pub resolution: String,    // пусто = не менять разрешение, иначе "1920x1080"
    pub hw_decode: bool,
    pub loudnorm: bool,        // нормализация громкости (-af loudnorm), только при перекодировании звука
    pub faststart: bool,       // -movflags +faststart, только для mp4/mov/m4a
    pub keep_metadata: bool,   // -map_metadata 0 (копировать метаданные исходника)
    pub threads: u32,          // -threads N (0 = авто)
}

impl Default for JobSettings {
    fn default() -> Self {
        Self {
            video_codec: "libx264".into(),
            audio_codec: "aac".into(),
            crf: 23,
            video_bitrate: String::new(),
            audio_bitrate: "192k".into(),
            preset: "medium".into(),
            resolution: String::new(),
            hw_decode: false,
            loudnorm: false,
            faststart: false,
            keep_metadata: false,
            threads: 0,
        }
    }
}

pub struct QueueItem {
    pub id: u64,
    pub input: PathBuf,
    pub output: PathBuf,
    pub settings: JobSettings,
}

/// Краткая информация об исходном файле для отображения в карточке.
#[derive(Debug, Clone, Default)]
pub struct MediaInfo {
    pub duration: Option<f64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub v_codec: Option<String>,
    pub a_codec: Option<String>,
    pub size_bytes: Option<u64>,
}

/// Опрашивает файл через ffprobe: разрешение, кодеки, длительность, размер.
/// При любой ошибке возвращает частично заполненную (или пустую) структуру.
pub fn probe_info(input: &Path, ffprobe: &str) -> MediaInfo {
    let mut info = MediaInfo {
        duration: probe_duration(input, ffprobe),
        size_bytes: std::fs::metadata(input).ok().map(|m| m.len()),
        ..Default::default()
    };

    // По блоку [STREAM]…[/STREAM] на каждый поток, поля в виде key=value
    // (порядок ключей у ffprobe внутренний, поэтому позиционный разбор не годится).
    let mut cmd = Command::new(ffprobe_exe(ffprobe));
    cmd.args([
        "-v", "error",
        "-show_entries", "stream=codec_type,codec_name,width,height",
        "-of", "default",
    ])
    .arg(input);
    hide_window(&mut cmd);
    let streams = cmd.output();

    if let Ok(out) = streams {
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout);
            let (mut codec_type, mut codec_name, mut width, mut height) =
                (String::new(), String::new(), None, None);

            for line in text.lines() {
                let line = line.trim();
                if line == "[/STREAM]" {
                    match codec_type.as_str() {
                        "video" if info.v_codec.is_none() => {
                            info.v_codec = (!codec_name.is_empty()).then(|| codec_name.clone());
                            info.width = width;
                            info.height = height;
                        }
                        "audio" if info.a_codec.is_none() => {
                            info.a_codec = (!codec_name.is_empty()).then(|| codec_name.clone());
                        }
                        _ => {}
                    }
                    codec_type.clear();
                    codec_name.clear();
                    width = None;
                    height = None;
                } else if let Some((key, value)) = line.split_once('=') {
                    match key {
                        "codec_type" => codec_type = value.to_string(),
                        "codec_name" => codec_name = value.to_string(),
                        "width" => width = value.parse().ok(),
                        "height" => height = value.parse().ok(),
                        _ => {}
                    }
                }
            }
        }
    }

    info
}

#[derive(Debug, Clone)]
pub enum Event {
    Started(u64),
    Progress(u64, f32),
    Done(u64),
    Failed(u64, String),
    QueueFinished,
}

/// Подбирает путь для результата. `template` задаёт имя (плейсхолдер `{name}` — имя исходника).
/// При `overwrite=false` не перезаписывает существующий файл (добавляет счётчик);
/// при `overwrite=true` разрешает существующий, но никогда не совпадает с самим исходником.
pub fn compute_output_path(
    input: &Path,
    ext: &str,
    output_dir: Option<&Path>,
    template: &str,
    overwrite: bool,
) -> PathBuf {
    let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("output");
    let name = if template.contains("{name}") {
        template.replace("{name}", stem)
    } else if template.trim().is_empty() {
        stem.to_string()
    } else {
        format!("{stem}{template}")
    };
    let dir = output_dir.map(|d| d.to_path_buf()).unwrap_or_else(|| {
        input.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."))
    });

    let base = dir.join(format!("{name}.{ext}"));
    if overwrite {
        if base != *input {
            return base;
        }
    } else if base != *input && !base.exists() {
        return base;
    }

    let mut counter = 1u32;
    loop {
        let candidate = dir.join(format!("{name}_{counter}.{ext}"));
        if candidate != *input && !candidate.exists() {
            return candidate;
        }
        counter += 1;
    }
}

fn ffprobe_exe(ffprobe: &str) -> &str {
    if ffprobe.trim().is_empty() {
        "ffprobe"
    } else {
        ffprobe.trim()
    }
}

fn probe_duration(input: &Path, ffprobe: &str) -> Option<f64> {
    let mut cmd = Command::new(ffprobe_exe(ffprobe));
    cmd.args([
        "-v", "error",
        "-show_entries", "format=duration",
        "-of", "default=noprint_wrappers=1:nokey=1",
    ])
    .arg(input);
    hide_window(&mut cmd);
    let output = cmd.output().ok()?;

    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout).trim().parse::<f64>().ok()
}

fn parse_time_to_secs(s: &str) -> Option<f64> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    let hours: f64 = parts[0].parse().ok()?;
    let minutes: f64 = parts[1].parse().ok()?;
    let seconds: f64 = parts[2].parse().ok()?;
    Some(hours * 3600.0 + minutes * 60.0 + seconds)
}

fn build_ffmpeg_args(input: &Path, output: &Path, s: &JobSettings) -> Vec<String> {
    let mut args: Vec<String> = vec!["-y".into()];

    if s.hw_decode {
        args.push("-hwaccel".into());
        args.push("cuda".into());
    }

    args.push("-i".into());
    args.push(input.to_string_lossy().to_string());

    if s.keep_metadata {
        args.push("-map_metadata".into());
        args.push("0".into());
    }
    if s.threads > 0 {
        args.push("-threads".into());
        args.push(s.threads.to_string());
    }

    match s.video_codec.as_str() {
        "none" => args.push("-vn".into()),
        codec => {
            args.push("-c:v".into());
            args.push(codec.to_string());

            let is_nvenc = codec.ends_with("_nvenc");

            if !s.resolution.trim().is_empty() {
                args.push("-vf".into());
                args.push(format!("scale={}", s.resolution.replace('x', ":")));
            }

            if !s.preset.trim().is_empty() {
                args.push("-preset".into());
                args.push(s.preset.clone());
            }

            if !s.video_bitrate.trim().is_empty() {
                args.push("-b:v".into());
                args.push(s.video_bitrate.clone());
            } else if !is_nvenc {
                args.push("-crf".into());
                args.push(s.crf.to_string());
            }
        }
    }

    match s.audio_codec.as_str() {
        "none" => args.push("-an".into()),
        codec => {
            args.push("-c:a".into());
            args.push(codec.to_string());
            if !s.audio_bitrate.trim().is_empty() {
                args.push("-b:a".into());
                args.push(s.audio_bitrate.clone());
            }
            // Фильтры несовместимы с потоковым копированием (-c:a copy).
            if s.loudnorm && codec != "copy" {
                args.push("-af".into());
                args.push("loudnorm".into());
            }
        }
    }

    // +faststart переносит индекс (moov atom) в начало файла для быстрой отдачи в вебе.
    // Опция принадлежит mp4/mov-муксеру; для остальных контейнеров ffmpeg её отвергает.
    let ext = output.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if s.faststart && matches!(ext.as_str(), "mp4" | "mov" | "m4a") {
        args.push("-movflags".into());
        args.push("+faststart".into());
    }

    args.push(output.to_string_lossy().to_string());
    args
}

/// Выполняет очередь job'ов последовательно в текущем (фоновом) потоке,
/// отправляя события прогресса через `tx`. Останавливается, если `stop_flag`
/// станет true между job'ами; текущий процесс можно прервать через `current_child`.
pub fn run_queue(
    items: Vec<QueueItem>,
    tx: Sender<Event>,
    stop_flag: Arc<AtomicBool>,
    current_child: Arc<Mutex<Option<Child>>>,
    ffmpeg_path: String,
    ffprobe_path: String,
    low_priority: bool,
) {
    let ffmpeg_exe = if ffmpeg_path.trim().is_empty() {
        "ffmpeg".to_string()
    } else {
        ffmpeg_path.trim().to_string()
    };

    for item in items {
        if stop_flag.load(Ordering::SeqCst) {
            break;
        }

        let duration = probe_duration(&item.input, &ffprobe_path);
        let args = build_ffmpeg_args(&item.input, &item.output, &item.settings);

        let mut cmd = Command::new(&ffmpeg_exe);
        cmd.args(&args).stdout(Stdio::null()).stderr(Stdio::piped());
        hide_window_prio(&mut cmd, low_priority);
        let child = cmd.spawn();

        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(Event::Failed(item.id, format!("не удалось запустить ffmpeg: {e}")));
                continue;
            }
        };

        let _ = tx.send(Event::Started(item.id));

        if let Some(stderr) = child.stderr.take() {
            let reader = BufReader::new(stderr);
            for line in reader.lines().flatten() {
                if stop_flag.load(Ordering::SeqCst) {
                    break;
                }
                if let Some(pos) = line.find("time=") {
                    let rest = &line[pos + 5..];
                    let time_str = rest.split_whitespace().next().unwrap_or("");
                    if let Some(secs) = parse_time_to_secs(time_str) {
                        if let Some(d) = duration {
                            if d > 0.0 {
                                let pct = ((secs / d) * 100.0).clamp(0.0, 100.0) as f32;
                                let _ = tx.send(Event::Progress(item.id, pct));
                            }
                        }
                    }
                }
            }
        }

        *current_child.lock().unwrap() = Some(child);
        let status = {
            let mut guard = current_child.lock().unwrap();
            guard.as_mut().unwrap().wait()
        };
        *current_child.lock().unwrap() = None;

        match status {
            Ok(st) if st.success() => {
                let _ = tx.send(Event::Done(item.id));
            }
            Ok(st) => {
                let _ = tx.send(Event::Failed(item.id, format!("ffmpeg завершился с кодом {:?}", st.code())));
            }
            Err(e) => {
                let _ = tx.send(Event::Failed(item.id, format!("ошибка ожидания процесса: {e}")));
            }
        }
    }

    let _ = tx.send(Event::QueueFinished);
}
