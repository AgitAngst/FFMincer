// Состояние приложения и всё, что происходит вне отрисовки: очередь ffmpeg, пресеты,
// установка ffmpeg, настройки и обновления. Рисует окно `ui.rs`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use anvil_ui::Accent;
use eframe::egui;

use crate::config::{self, AppConfig, PostAction};
use crate::ffmpeg::{self, Event, JobSettings, MediaInfo, QueueItem};
use crate::texts::Preset;

/// Розовый — «фарш» FFMincer.
pub const ACCENT: Accent = Accent::ROSE;

pub const VIDEO_CODECS: [&str; 6] = ["libx264", "libx265", "h264_nvenc", "hevc_nvenc", "copy", "none"];
pub const AUDIO_CODECS: [&str; 6] = ["aac", "libmp3lame", "libopus", "flac", "copy", "none"];
pub const CONTAINERS: [&str; 8] = ["mp4", "mkv", "mov", "webm", "mp3", "m4a", "opus", "flac"];
pub const MEDIA_EXTENSIONS: [&str; 16] =
    ["mp4", "mkv", "mov", "avi", "webm", "flv", "m4v", "wmv", "mp3", "wav", "flac", "aac", "ogg", "opus", "m4a", "wma"];

#[derive(Debug, Clone)]
pub enum JobStatus {
    Pending,
    Running(f32),
    Done,
    Failed(String),
}

pub struct Job {
    pub id: u64,
    pub input: PathBuf,
    pub output: PathBuf,
    pub status: JobStatus,
    pub info: MediaInfo,
}

pub struct App {
    pub jobs: Vec<Job>,
    next_id: u64,

    pub settings: JobSettings,
    pub container_ext: String,
    pub output_dir: Option<PathBuf>,
    pub current_preset: Option<Preset>,

    pub config: AppConfig,
    pub updater: anvil_update::Updater,
    pub startup_size_sent: bool,
    pub startup_shown: bool,
    pub startup_target_h: f32,
    pub startup_frames: u32,

    pub show_settings: bool,
    pub show_about: bool,
    pub autostart_cached: Option<bool>,
    pub ffmpeg_version: Option<String>, // результат последней проверки (None = не найден/не проверяли)
    pub ffmpeg_checked: bool,
    pub installing: bool,
    install_rx: Option<Receiver<Result<(), String>>>,
    pub install_msg: String,

    pub processing: bool,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    stop_flag: Arc<AtomicBool>,
    current_child: Arc<Mutex<Option<std::process::Child>>>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let config = config::load();
        anvil_ui::install(&cc.egui_ctx, ACCENT, config.common.theme);
        config.common.apply(&cc.egui_ctx);
        let updater = anvil_update::Updater::new(
            anvil_update::Config::new("ffmincer", env!("CARGO_PKG_VERSION"), "AgitAngst/FFMincer"),
            cc.egui_ctx.clone(),
        );
        // Если геометрия восстановлена из конфига — окно уже открыто нужного размера,
        // подгонять и заново показывать не нужно.
        let has_geometry = config.geometry.is_some();
        let output_dir =
            if config.output_dir.trim().is_empty() { None } else { Some(PathBuf::from(&config.output_dir)) };
        let settings =
            JobSettings { threads: config.threads, keep_metadata: config.keep_metadata, ..Default::default() };
        let (tx, rx) = std::sync::mpsc::channel();
        Self {
            jobs: Vec::new(),
            next_id: 1,
            settings,
            container_ext: "mp4".into(),
            output_dir,
            current_preset: None,
            config,
            updater,
            startup_size_sent: has_geometry,
            startup_shown: has_geometry,
            startup_target_h: 0.0,
            startup_frames: 0,
            show_settings: false,
            show_about: false,
            autostart_cached: None,
            ffmpeg_version: None,
            ffmpeg_checked: false,
            installing: false,
            install_rx: None,
            install_msg: String::new(),
            processing: false,
            tx,
            rx,
            stop_flag: Arc::new(AtomicBool::new(false)),
            current_child: Arc::new(Mutex::new(None)),
        }
    }

    pub fn lang(&self) -> anvil_ui::Lang {
        self.config.common.language
    }

    pub fn add_file(&mut self, path: PathBuf) {
        if !path.is_file() {
            return;
        }
        let output = ffmpeg::compute_output_path(
            &path,
            &self.container_ext,
            self.output_dir.as_deref(),
            &self.config.name_template,
            self.config.overwrite,
        );
        let info = ffmpeg::probe_info(&path, &self.config.ffprobe_path);
        self.jobs.push(Job { id: self.next_id, input: path, output, status: JobStatus::Pending, info });
        self.next_id += 1;
    }

    /// Добавить файлы в очередь; если так настроено — сразу запустить.
    pub fn add_files(&mut self, paths: Vec<PathBuf>) {
        let added = !paths.is_empty();
        for path in paths {
            self.add_file(path);
        }
        if added && self.config.autostart_queue && !self.processing {
            self.start_queue();
        }
    }

    pub fn apply_preset(&mut self, preset: Preset) {
        match preset {
            Preset::Mp4Cpu => {
                self.settings.video_codec = "libx264".into();
                self.settings.audio_codec = "aac".into();
                self.settings.crf = 23;
                self.settings.video_bitrate.clear();
                self.settings.audio_bitrate = "192k".into();
                self.settings.preset = "medium".into();
                self.settings.hw_decode = false;
                self.container_ext = "mp4".into();
            }
            Preset::Mp4Nvenc => {
                self.settings.video_codec = "h264_nvenc".into();
                self.settings.audio_codec = "aac".into();
                self.settings.video_bitrate = "8M".into();
                self.settings.audio_bitrate = "192k".into();
                self.settings.preset = "p5".into();
                self.settings.hw_decode = true;
                self.container_ext = "mp4".into();
            }
            Preset::Mkv265 => {
                self.settings.video_codec = "libx265".into();
                self.settings.audio_codec = "aac".into();
                self.settings.crf = 24;
                self.settings.video_bitrate.clear();
                self.settings.audio_bitrate = "192k".into();
                self.settings.preset = "slow".into();
                self.settings.hw_decode = false;
                self.container_ext = "mkv".into();
            }
            Preset::Mp3 => {
                self.settings.video_codec = "none".into();
                self.settings.audio_codec = "libmp3lame".into();
                self.settings.audio_bitrate = "256k".into();
                self.container_ext = "mp3".into();
            }
            Preset::Flac => {
                self.settings.video_codec = "none".into();
                self.settings.audio_codec = "flac".into();
                self.settings.audio_bitrate.clear();
                self.container_ext = "flac".into();
            }
        }
        // Пересчитать выходные пути для ещё не запущенных job'ов под новый контейнер.
        self.recompute_pending_outputs();
    }

    pub fn recompute_pending_outputs(&mut self) {
        for job in self.jobs.iter_mut() {
            if matches!(job.status, JobStatus::Pending) {
                job.output = ffmpeg::compute_output_path(
                    &job.input,
                    &self.container_ext,
                    self.output_dir.as_deref(),
                    &self.config.name_template,
                    self.config.overwrite,
                );
            }
        }
    }

    fn drain_events(&mut self) {
        let mut finished = false;
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::Started(id) => self.set_status(id, JobStatus::Running(0.0)),
                Event::Progress(id, pct) => self.set_status(id, JobStatus::Running(pct)),
                Event::Done(id) => {
                    self.set_status(id, JobStatus::Done);
                    if self.config.autoclear_finished {
                        self.jobs.retain(|j| j.id != id);
                    }
                }
                Event::Failed(id, msg) => self.set_status(id, JobStatus::Failed(msg)),
                Event::QueueFinished => {
                    self.processing = false;
                    finished = true;
                }
            }
        }
        if finished {
            self.on_queue_finished();
        }
    }

    fn output_folder_for_action(&self) -> Option<PathBuf> {
        if let Some(dir) = &self.output_dir {
            return Some(dir.clone());
        }
        self.jobs.iter().find_map(|j| j.output.parent().map(|p| p.to_path_buf()))
    }

    fn on_queue_finished(&mut self) {
        if self.config.sound_on_finish {
            crate::sys::play_finish_sound();
        }
        match self.config.post_action {
            PostAction::None => {}
            PostAction::OpenFolder => {
                if let Some(dir) = self.output_folder_for_action() {
                    crate::sys::open_folder(&dir);
                }
            }
            PostAction::Sleep => crate::sys::sleep_pc(),
            PostAction::Shutdown => crate::sys::shutdown_pc(),
        }
    }

    fn set_status(&mut self, id: u64, status: JobStatus) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
            job.status = status;
        }
    }

    pub fn start_queue(&mut self) {
        // Настройки из окна настроек, влияющие на команду ffmpeg.
        self.settings.threads = self.config.threads;
        self.settings.keep_metadata = self.config.keep_metadata;

        let items: Vec<QueueItem> = self
            .jobs
            .iter()
            .filter(|j| matches!(j.status, JobStatus::Pending))
            .map(|j| QueueItem {
                id: j.id,
                input: j.input.clone(),
                output: j.output.clone(),
                settings: self.settings.clone(),
            })
            .collect();

        if items.is_empty() {
            return;
        }

        self.processing = true;
        self.stop_flag.store(false, Ordering::SeqCst);

        let tx = self.tx.clone();
        let stop_flag = self.stop_flag.clone();
        let current_child = self.current_child.clone();
        let ffmpeg_path = self.config.ffmpeg_path.clone();
        let ffprobe_path = self.config.ffprobe_path.clone();
        let low_priority = self.config.low_priority;
        std::thread::spawn(move || {
            ffmpeg::run_queue(items, tx, stop_flag, current_child, ffmpeg_path, ffprobe_path, low_priority);
        });
    }

    pub fn cancel_queue(&mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        if let Ok(mut guard) = self.current_child.lock()
            && let Some(child) = guard.as_mut()
        {
            let _ = child.kill();
        }
    }

    pub fn clear_finished(&mut self) {
        self.jobs.retain(|j| !matches!(j.status, JobStatus::Done));
    }

    pub fn persist(&self) {
        config::save(&self.config);
    }

    pub fn start_ffmpeg_install(&mut self) {
        if self.installing {
            return;
        }
        self.installing = true;
        self.install_msg.clear();
        let (tx, rx) = std::sync::mpsc::channel();
        self.install_rx = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(crate::sys::install_ffmpeg());
        });
    }

    pub fn check_ffmpeg(&mut self) {
        self.ffmpeg_version = crate::sys::ffmpeg_version(&self.config.ffmpeg_path);
        self.ffmpeg_checked = true;
    }

    /// Всё, что не рисование: события очереди, установка ffmpeg, брошенные файлы, обновления.
    pub fn tick(&mut self, ctx: &egui::Context) {
        // Результат фоновой установки ffmpeg.
        if let Some(rx) = &self.install_rx
            && let Ok(res) = rx.try_recv()
        {
            self.installing = false;
            self.install_rx = None;
            self.install_msg = match res {
                Ok(()) => crate::texts::tip(self.lang(), "ffmpeg установлен", "ffmpeg installed").to_owned(),
                Err(err) => err,
            };
            self.check_ffmpeg();
        }
        if self.installing {
            ctx.request_repaint_after(std::time::Duration::from_millis(300));
        }

        self.drain_events();
        if self.processing {
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }
        // Пока окно ещё не показано — гоним кадры, чтобы гарантированно дойти до замера и показа.
        if !self.startup_shown {
            ctx.request_repaint();
        }

        // Drag-and-drop файлов из проводника.
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if !dropped.is_empty() {
            self.add_files(dropped);
        }

        self.updater.auto(&self.config.common);
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.tick(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        crate::ui::draw(self, ui);
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        anvil_ui::chrome::clear_color(visuals, ACCENT)
    }
}
