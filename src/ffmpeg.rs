// Логика работы с ffmpeg: настройки job'а, сборка аргументов, выполнение очереди
// в фоновом потоке с отправкой событий прогресса в UI через mpsc::channel.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use crate::sys::resolve_tool;

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

/// Удалить недописанный файл. Только что убитый процесс на Windows отпускает файл не мгновенно —
/// пробуем несколько раз (до ~3 с), а не сдаёмся на первой ошибке доступа.
fn remove_partial(path: &Path) {
    for _ in 0..30 {
        match std::fs::remove_file(path) {
            Ok(()) => return,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(100)),
        }
    }
}

/// Прервать ffmpeg вместе с его дочерними процессами. Простого `Child::kill` мало: если ffmpeg
/// запускается через шим (Chocolatey, scoop), убился бы только шим, а настоящий ffmpeg продолжил
/// бы работать и писать файл.
pub fn kill_process_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("taskkill");
        cmd.args(["/PID", &child.id().to_string(), "/T", "/F"]);
        hide_window(&mut cmd);
        let _ = cmd.output();
    }
    let _ = child.kill();
}

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
    pub loudnorm: bool,      // нормализация громкости (-af loudnorm), только при перекодировании звука
    pub faststart: bool,     // -movflags +faststart, только для mp4/mov/m4a
    pub keep_metadata: bool, // -map_metadata 0 (копировать метаданные исходника)
    pub threads: u32,        // -threads N (0 = авто)
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

/// Какие значения `-preset` понимает кодек. У CPU-кодеков x264/x265 это ultrafast…placebo, у NVENC —
/// p1…p7 (и старые имена); чужое значение ffmpeg отвергает («invalid preset»).
pub fn preset_fits(codec: &str, preset: &str) -> bool {
    const X26X: [&str; 10] =
        ["ultrafast", "superfast", "veryfast", "faster", "fast", "medium", "slow", "slower", "veryslow", "placebo"];
    let preset = preset.trim();
    if preset.is_empty() {
        return true;
    }
    if codec.ends_with("_nvenc") {
        let modern = preset.len() == 2 && preset.starts_with('p') && matches!(preset.as_bytes()[1], b'1'..=b'7');
        modern || matches!(preset, "default" | "slow" | "medium" | "fast" | "hp" | "hq")
    } else if codec.starts_with("libx26") {
        X26X.contains(&preset)
    } else {
        true
    }
}

/// Значение `-preset` по умолчанию для кодека (пусто, если у кодека пресетов нет).
pub fn default_preset(codec: &str) -> &'static str {
    if codec.ends_with("_nvenc") {
        "p5"
    } else if codec.starts_with("libx26") {
        "medium"
    } else {
        ""
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
    let mut cmd = Command::new(resolve_tool(ffprobe, "ffprobe"));
    cmd.args(["-v", "error", "-show_entries", "stream=codec_type,codec_name,width,height", "-of", "default"])
        .arg(input);
    hide_window(&mut cmd);
    let streams = cmd.output();

    if let Ok(out) = streams
        && out.status.success()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        let (mut codec_type, mut codec_name, mut width, mut height) = (String::new(), String::new(), None, None);

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

    info
}

/// Опрос файла в фоне: окно не замирает, пока ffprobe читает большие файлы или их много.
pub fn spawn_probe(id: u64, path: PathBuf, ffprobe: String, tx: Sender<Event>) {
    std::thread::spawn(move || {
        let _ = tx.send(Event::Info(id, probe_info(&path, &ffprobe)));
    });
}

/// Почему задание не выполнено. Текст собирает интерфейс — на языке пользователя.
#[derive(Debug, Clone)]
pub enum Failure {
    /// ffmpeg не запустился (нет в PATH, неверный путь).
    Spawn(String),
    /// ffmpeg завершился с ошибкой: код и последние строки его сообщений.
    Exit(Option<i32>, String),
}

#[derive(Debug, Clone)]
pub enum Event {
    Started(u64),
    Progress(u64, f32),
    Info(u64, MediaInfo),
    Done(u64),
    Failed(u64, Failure),
    /// Пользователь отменил: задание возвращается в ожидание, недописанный файл убран.
    Cancelled(u64),
    QueueFinished,
}

/// Имя файла без символов, которые Windows не принимает (`<>:"/\|?*`), и без точек и пробелов
/// в конце — иначе шаблон вроде `{name}: маленький` давал бы ошибку ffmpeg.
fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim_end_matches(['.', ' ']).to_string();
    if cleaned.is_empty() { "output".into() } else { cleaned }
}

/// Подбирает путь для результата. `template` задаёт имя (плейсхолдер `{name}` — имя исходника).
/// `reserved` — выходные пути других заданий очереди: два задания никогда не пишут в один файл.
/// При `overwrite=false` не перезаписывает существующий файл (добавляет счётчик);
/// при `overwrite=true` разрешает существующий, но никогда не совпадает с самим исходником.
pub fn compute_output_path(
    input: &Path,
    ext: &str,
    output_dir: Option<&Path>,
    template: &str,
    overwrite: bool,
    reserved: &[PathBuf],
) -> PathBuf {
    let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("output");
    let name = if template.contains("{name}") {
        template.replace("{name}", stem)
    } else if template.trim().is_empty() {
        stem.to_string()
    } else {
        format!("{stem}{template}")
    };
    let name = sanitize_file_name(&name);
    let dir = output_dir
        .map(|d| d.to_path_buf())
        .unwrap_or_else(|| input.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from(".")));

    let free =
        |candidate: &PathBuf| candidate != input && !reserved.contains(candidate) && (overwrite || !candidate.exists());
    let base = dir.join(format!("{name}.{ext}"));
    if free(&base) {
        return base;
    }
    let mut counter = 1u32;
    loop {
        let candidate = dir.join(format!("{name}_{counter}.{ext}"));
        if free(&candidate) {
            return candidate;
        }
        counter += 1;
    }
}

fn probe_duration(input: &Path, ffprobe: &str) -> Option<f64> {
    let mut cmd = Command::new(resolve_tool(ffprobe, "ffprobe"));
    cmd.args(["-v", "error", "-show_entries", "format=duration", "-of", "default=noprint_wrappers=1:nokey=1"])
        .arg(input);
    hide_window(&mut cmd);
    let output = cmd.output().ok()?;

    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout).trim().parse::<f64>().ok()
}

/// Секунды обработанного из строки `-progress` (`out_time_us=1234567`); прочие строки и `N/A` — `None`.
fn parse_progress_line(line: &str) -> Option<f64> {
    let value = line.trim().strip_prefix("out_time_us=")?;
    let micros: f64 = value.parse().ok()?;
    (micros >= 0.0).then_some(micros / 1_000_000.0)
}

fn build_ffmpeg_args(input: &Path, output: &Path, s: &JobSettings) -> Vec<String> {
    // Прогресс — машинный (`-progress pipe:1`, строки key=value): stderr ffmpeg разделяет свои
    // строки статуса символом \r, и читать по строкам его нельзя. Сообщения только об ошибках.
    let mut args: Vec<String> =
        ["-y", "-nostdin", "-hide_banner", "-loglevel", "error", "-nostats", "-progress", "pipe:1"]
            .iter()
            .map(|a| a.to_string())
            .collect();

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

            // Фильтр и потоковое копирование несовместимы: ffmpeg отказывается открывать выход.
            if codec != "copy" && !s.resolution.trim().is_empty() {
                args.push("-vf".into());
                args.push(format!("scale={}", s.resolution.trim().to_ascii_lowercase().replace('x', ":")));
            }

            // Чужой пресет (p5 у libx264 и наоборот) — ошибка «invalid preset», поэтому не передаём.
            if !s.preset.trim().is_empty() && preset_fits(codec, &s.preset) {
                args.push("-preset".into());
                args.push(s.preset.trim().to_string());
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
    let ffmpeg_exe = resolve_tool(&ffmpeg_path, "ffmpeg");

    for item in items {
        if stop_flag.load(Ordering::SeqCst) {
            break;
        }

        let duration = probe_duration(&item.input, &ffprobe_path);
        let args = build_ffmpeg_args(&item.input, &item.output, &item.settings);
        // Недописанный файл убираем, только если его не было до нас: при «перезаписывать» рядом мог
        // лежать прежний результат, и удалять его нельзя.
        let output_existed = item.output.exists();

        let mut cmd = Command::new(&ffmpeg_exe);
        cmd.args(&args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        hide_window_prio(&mut cmd, low_priority);

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(Event::Failed(item.id, Failure::Spawn(e.to_string())));
                continue;
            }
        };
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        // Процесс сразу становится доступен для «Отменить» — а не после чтения всего вывода.
        {
            let mut slot = current_child.lock().unwrap();
            // Отмену нажали, пока ffmpeg запускался.
            if stop_flag.load(Ordering::SeqCst) {
                kill_process_tree(&mut child);
            }
            *slot = Some(child);
        }
        let _ = tx.send(Event::Started(item.id));

        // stderr читаем отдельно (иначе переполненная труба остановит ffmpeg) и храним хвост:
        // это и есть причина ошибки для пользователя.
        let tail = Arc::new(Mutex::new(VecDeque::<String>::new()));
        let tail_reader = stderr.map(|stderr| {
            let tail = tail.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    let mut tail = tail.lock().unwrap();
                    tail.push_back(line);
                    if tail.len() > 6 {
                        tail.pop_front();
                    }
                }
            })
        });

        if let Some(stdout) = stdout {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(secs) = parse_progress_line(&line)
                    && let Some(d) = duration
                    && d > 0.0
                {
                    let pct = ((secs / d) * 100.0).clamp(0.0, 100.0) as f32;
                    let _ = tx.send(Event::Progress(item.id, pct));
                }
            }
        }

        // Труба закрылась — процесс завершён (или убит отменой). Ждём его уже без замка,
        // иначе кнопка «Отменить» зависла бы на этом замке.
        let child = current_child.lock().unwrap().take();
        let status = match child {
            Some(mut c) => c.wait(),
            None => Err(std::io::Error::other("process handle lost")),
        };
        if let Some(reader) = tail_reader {
            let _ = reader.join();
        }
        let cancelled = stop_flag.load(Ordering::SeqCst);

        match status {
            Ok(st) if st.success() => {
                let _ = tx.send(Event::Done(item.id));
            }
            other => {
                if !output_existed {
                    remove_partial(&item.output);
                }
                if cancelled {
                    let _ = tx.send(Event::Cancelled(item.id));
                } else {
                    let message = tail.lock().unwrap().iter().cloned().collect::<Vec<_>>().join("\n");
                    let code = other.ok().and_then(|st| st.code());
                    let _ = tx.send(Event::Failed(item.id, Failure::Exit(code, message)));
                }
            }
        }
    }

    let _ = tx.send(Event::QueueFinished);
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::channel;
    use std::time::{Duration, Instant};

    use super::*;

    fn args_for(s: &JobSettings, out: &str) -> Vec<String> {
        build_ffmpeg_args(Path::new("in.mkv"), Path::new(out), s)
    }

    #[test]
    fn progress_is_machine_readable_and_quiet() {
        let a = args_for(&JobSettings::default(), "out.mp4");
        let pos = a.iter().position(|x| x == "-progress").expect("нужен -progress");
        assert_eq!(a[pos + 1], "pipe:1");
        assert!(a.contains(&"-nostats".to_string()) && a.contains(&"-nostdin".to_string()));
        assert_eq!(a.last().unwrap(), "out.mp4", "выходной файл — последним");
    }

    #[test]
    fn copy_does_not_get_a_filter() {
        let s = JobSettings { video_codec: "copy".into(), resolution: "1920x1080".into(), ..Default::default() };
        assert!(!args_for(&s, "o.mp4").contains(&"-vf".to_string()), "фильтр с copy ffmpeg не принимает");
        let s = JobSettings { resolution: "1920X1080".into(), ..Default::default() };
        let a = args_for(&s, "o.mp4");
        assert_eq!(a[a.iter().position(|x| x == "-vf").unwrap() + 1], "scale=1920:1080", "X в любом регистре");
    }

    #[test]
    fn foreign_preset_is_not_passed() {
        let x264 = JobSettings { video_codec: "libx264".into(), preset: "p5".into(), ..Default::default() };
        assert!(!args_for(&x264, "o.mp4").contains(&"-preset".to_string()));
        let nv = JobSettings { video_codec: "h264_nvenc".into(), preset: "p5".into(), ..Default::default() };
        assert!(args_for(&nv, "o.mp4").contains(&"p5".to_string()));
        assert!(
            preset_fits("libx265", "slow") && !preset_fits("libx265", "p3") && !preset_fits("hevc_nvenc", "veryslow")
        );
        assert_eq!(
            (default_preset("h264_nvenc"), default_preset("libx264"), default_preset("copy")),
            ("p5", "medium", "")
        );
    }

    #[test]
    fn loudnorm_and_faststart_only_where_they_work() {
        let s = JobSettings { audio_codec: "copy".into(), loudnorm: true, faststart: true, ..Default::default() };
        assert!(!args_for(&s, "o.mp4").contains(&"-af".to_string()), "loudnorm с copy невозможен");
        assert!(args_for(&s, "o.mp4").contains(&"+faststart".to_string()));
        assert!(!args_for(&s, "o.mkv").contains(&"+faststart".to_string()), "faststart только mp4/mov/m4a");
    }

    #[test]
    fn progress_line_parsing() {
        assert_eq!(parse_progress_line("out_time_us=2500000"), Some(2.5));
        assert_eq!(parse_progress_line("out_time_us=N/A"), None);
        assert_eq!(parse_progress_line("out_time_us=-9223372036854775807"), None);
        assert_eq!(parse_progress_line("progress=continue"), None);
    }

    #[test]
    fn output_names_never_collide() {
        let dir = std::env::temp_dir().join(format!("ffmincer-names-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("clip.mkv");
        let b = dir.join("clip.avi");
        // Первое задание заняло clip.mp4 — второе с тем же именем получает другое.
        let first = compute_output_path(&a, "mp4", None, "{name}", false, &[]);
        let second = compute_output_path(&b, "mp4", None, "{name}", false, std::slice::from_ref(&first));
        assert_eq!(first, dir.join("clip.mp4"));
        assert_ne!(first, second);
        // Даже «перезаписывать» не отправляет два задания в один файл.
        let third = compute_output_path(&b, "mp4", None, "{name}", true, std::slice::from_ref(&first));
        assert_ne!(first, third);
        // Исходник не затирается никогда.
        let same = dir.join("clip.mp4");
        assert_ne!(compute_output_path(&same, "mp4", None, "{name}", true, &[]), same);
        // Символы, недопустимые в имени, заменяются.
        let odd = compute_output_path(&a, "mp4", None, "{name}: мал?", false, &[]);
        assert_eq!(odd.file_name().unwrap().to_string_lossy(), "clip_ мал_.mp4");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn ffmpeg_available() -> bool {
        Command::new("ffmpeg").arg("-version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok()
    }

    /// Делает тестовый ролик в `dir`, возвращает путь; `secs` — длина.
    fn make_clip(dir: &Path, secs: u32) -> PathBuf {
        let path = dir.join("src.mp4");
        let status = Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", &format!("testsrc=duration={secs}:size=640x360:rate=30")])
            .args(["-f", "lavfi", "-i", &format!("sine=frequency=440:duration={secs}"), "-shortest"])
            .args(["-c:v", "libx264", "-c:a", "aac"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        path
    }

    fn run(item: QueueItem, stop: Arc<AtomicBool>) -> (Vec<Event>, Duration) {
        let (tx, rx) = channel();
        let child = Arc::new(Mutex::new(None));
        let started = Instant::now();
        run_queue(vec![item], tx, stop, child, String::new(), String::new(), false);
        (rx.try_iter().collect(), started.elapsed())
    }

    #[test]
    fn queue_reports_progress_many_times_and_finishes() {
        if !ffmpeg_available() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("ffmincer-run-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = make_clip(&dir, 6);
        let output = dir.join("out.mp4");
        // Тяжёлый пресет, чтобы кодирование шло заметное время и успело дать несколько отсчётов.
        let settings = JobSettings { preset: "veryslow".into(), resolution: "1280x720".into(), ..Default::default() };
        let (events, _) =
            run(QueueItem { id: 7, input, output: output.clone(), settings }, Arc::new(AtomicBool::new(false)));
        let progress = events.iter().filter(|e| matches!(e, Event::Progress(7, _))).count();
        assert!(progress >= 3, "прогресс должен приходить по ходу работы, а не в конце: {progress} отсчётов");
        assert!(events.iter().any(|e| matches!(e, Event::Done(7))), "{events:?}");
        assert!(matches!(events.last(), Some(Event::QueueFinished)));
        assert!(output.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancel_stops_ffmpeg_and_removes_partial_file() {
        if !ffmpeg_available() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("ffmincer-cancel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Длинный ролик и медленный пресет: без отмены он шёл бы заведомо дольше проверки.
        let input = make_clip(&dir, 120);
        let output = dir.join("out.mp4");
        let settings = JobSettings { preset: "veryslow".into(), resolution: "1920x1080".into(), ..Default::default() };
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        let child = Arc::new(Mutex::new(None));
        let (s2, c2, o2) = (stop.clone(), child.clone(), output.clone());
        let worker = std::thread::spawn(move || {
            run_queue(
                vec![QueueItem { id: 1, input, output: o2, settings }],
                tx,
                s2,
                c2,
                String::new(),
                String::new(),
                false,
            );
        });
        // Ждём начала работы, затем делаем то же, что кнопка «Отменить».
        let deadline = Instant::now() + Duration::from_secs(20);
        while !matches!(rx.recv_timeout(Duration::from_secs(20)), Ok(Event::Progress(..))) {
            assert!(Instant::now() < deadline, "прогресс не пошёл");
        }
        let at = Instant::now();
        stop.store(true, Ordering::SeqCst);
        if let Some(c) = child.lock().unwrap().as_mut() {
            kill_process_tree(c);
        }
        worker.join().unwrap();
        assert!(at.elapsed() < Duration::from_secs(10), "отмена должна останавливать ffmpeg сразу: {:?}", at.elapsed());
        let rest: Vec<Event> = rx.try_iter().collect();
        assert!(rest.iter().any(|e| matches!(e, Event::Cancelled(1))), "{rest:?}");
        assert!(!output.exists(), "недописанный файл убран");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bad_input_reports_ffmpeg_message() {
        if !ffmpeg_available() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("ffmincer-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("not-a-video.mp4");
        std::fs::write(&input, b"this is not media").unwrap();
        let item = QueueItem { id: 3, input, output: dir.join("o.mp4"), settings: JobSettings::default() };
        let (events, _) = run(item, Arc::new(AtomicBool::new(false)));
        let failed = events.iter().find_map(|e| if let Event::Failed(3, f) = e { Some(f.clone()) } else { None });
        match failed {
            Some(Failure::Exit(Some(code), message)) => {
                assert_ne!(code, 0);
                assert!(!message.is_empty(), "причина ошибки от ffmpeg должна дойти до пользователя");
            }
            other => panic!("ожидалась Failure::Exit, получено {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
