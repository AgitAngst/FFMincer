// Логика работы с ffmpeg: настройки job'а, сборка аргументов, выполнение очереди
// в фоновом потоке с отправкой событий прогресса в UI через mpsc::channel.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

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
        }
    }
}

pub struct QueueItem {
    pub id: u64,
    pub input: PathBuf,
    pub output: PathBuf,
    pub settings: JobSettings,
}

#[derive(Debug, Clone)]
pub enum Event {
    Started(u64),
    Progress(u64, f32),
    Done(u64),
    Failed(u64, String),
    QueueFinished,
}

/// Подбирает свободный путь для результата: не совпадает со входным файлом
/// и не перезаписывает уже существующий файл на диске.
pub fn compute_output_path(input: &Path, ext: &str, output_dir: Option<&Path>) -> PathBuf {
    let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("output");
    let dir = output_dir.map(|d| d.to_path_buf()).unwrap_or_else(|| {
        input.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."))
    });

    let base = dir.join(format!("{stem}.{ext}"));
    if base != *input && !base.exists() {
        return base;
    }

    let mut counter = 1u32;
    loop {
        let candidate = dir.join(format!("{stem}_converted_{counter}.{ext}"));
        if candidate != *input && !candidate.exists() {
            return candidate;
        }
        counter += 1;
    }
}

fn probe_duration(input: &Path) -> Option<f64> {
    let output = Command::new("ffprobe")
        .args([
            "-v", "error",
            "-show_entries", "format=duration",
            "-of", "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(input)
        .output()
        .ok()?;

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
        }
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
) {
    for item in items {
        if stop_flag.load(Ordering::SeqCst) {
            break;
        }

        let duration = probe_duration(&item.input);
        let args = build_ffmpeg_args(&item.input, &item.output, &item.settings);

        let child = Command::new("ffmpeg")
            .args(&args)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn();

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
