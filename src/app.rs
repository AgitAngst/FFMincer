// Состояние приложения и всё, что происходит вне отрисовки: очередь ffmpeg, пресеты,
// установка ffmpeg, настройки и обновления. Рисует окно `ui.rs`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use anvil_ui::Accent;
use eframe::egui;

use crate::config::{self, AppConfig, PostAction};
use crate::ffmpeg::{self, Event, Failure, JobSettings, MediaInfo, QueueItem};
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
    Failed(Failure),
}

pub struct Job {
    pub id: u64,
    pub input: PathBuf,
    pub output: PathBuf,
    pub status: JobStatus,
    pub info: MediaInfo,
    /// ffprobe уже ответил (пока нет — в карточке скелетон вместо строки сведений).
    pub info_ready: bool,
    /// Ошибка случилась в этом кадре: строка «встряхивается» один раз (см. `ui::jobs`).
    pub shake: bool,
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
    /// Когда (время egui) размер или место окна изменились в последний раз; пишем в файл лишь после паузы.
    pub geometry_changed_at: Option<f64>,

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
        Self::build(&cc.egui_ctx, config::load())
    }

    /// Собрать состояние программы для контекста egui и настроек (без окна — так же собирают его тесты).
    fn build(ctx: &egui::Context, config: AppConfig) -> Self {
        anvil_ui::install(ctx, ACCENT, config.common.theme);
        config.common.apply(ctx);
        let updater = anvil_update::Updater::new(
            anvil_update::Config::new("ffmincer", env!("CARGO_PKG_VERSION"), "AgitAngst/FFMincer"),
            ctx.clone(),
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
            geometry_changed_at: None,
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
        // Тот же файл, что уже ждёт своей очереди, второй раз не добавляем.
        if self.jobs.iter().any(|j| j.input == path && matches!(j.status, JobStatus::Pending)) {
            return;
        }
        let reserved = self.reserved_outputs(None);
        let output = ffmpeg::compute_output_path(
            &path,
            &self.container_ext,
            self.output_dir.as_deref(),
            &self.config.name_template,
            self.config.overwrite,
            &reserved,
        );
        let id = self.next_id;
        self.next_id += 1;
        // ffprobe читает файл в фоне — окно не замирает, а сведения появляются в карточке следом.
        ffmpeg::spawn_probe(id, path.clone(), self.config.ffprobe_path.clone(), self.tx.clone());
        self.jobs.push(Job {
            id,
            input: path,
            output,
            status: JobStatus::Pending,
            info: MediaInfo::default(),
            info_ready: false,
            shake: false,
        });
    }

    /// Выходные пути очереди (кроме задания `except`): их нельзя занимать заданию, которому подбирают имя.
    fn reserved_outputs(&self, except: Option<u64>) -> Vec<PathBuf> {
        self.jobs.iter().filter(|j| Some(j.id) != except).map(|j| j.output.clone()).collect()
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
        // Имена подбираем по порядку очереди, каждое следующее учитывает уже занятые выше.
        let mut reserved: Vec<PathBuf> =
            self.jobs.iter().filter(|j| !matches!(j.status, JobStatus::Pending)).map(|j| j.output.clone()).collect();
        for job in self.jobs.iter_mut().filter(|j| matches!(j.status, JobStatus::Pending)) {
            job.output = ffmpeg::compute_output_path(
                &job.input,
                &self.container_ext,
                self.output_dir.as_deref(),
                &self.config.name_template,
                self.config.overwrite,
                &reserved,
            );
            reserved.push(job.output.clone());
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
                Event::Info(id, info) => {
                    if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
                        job.info = info;
                        job.info_ready = true;
                    }
                }
                Event::Failed(id, failure) => {
                    self.set_status(id, JobStatus::Failed(failure));
                    if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
                        job.shake = true;
                    }
                }
                // Отменённое задание снова ждёт: его можно запустить заново.
                Event::Cancelled(id) => self.set_status(id, JobStatus::Pending),
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
        // Очередь остановил сам пользователь — ни сна, ни выключения ПК, ни звука «всё готово».
        if self.stop_flag.load(Ordering::SeqCst) {
            return;
        }
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
            ffmpeg::kill_process_tree(child);
        }
    }

    /// Убрать из списка всё завершённое — и удачное, и с ошибкой.
    pub fn clear_finished(&mut self) {
        self.jobs.retain(|j| !matches!(j.status, JobStatus::Done | JobStatus::Failed(_)));
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
        // Флаг Windows «Показывать анимацию»: выключили — всё движение в окне станет мгновенным.
        anvil_ui::motion::tick(ctx);

        // Результат фоновой установки ffmpeg.
        if let Some(rx) = &self.install_rx
            && let Ok(res) = rx.try_recv()
        {
            self.installing = false;
            self.install_rx = None;
            self.check_ffmpeg();
            // winget мог вернуть код «уже установлен» — важно лишь, нашёлся ли ffmpeg теперь.
            self.install_msg = match (res, self.ffmpeg_version.is_some()) {
                (_, true) => crate::texts::tip(self.lang(), "ffmpeg готов к работе", "ffmpeg is ready").to_owned(),
                (Ok(()), false) => crate::texts::tip(
                    self.lang(),
                    "установлено, но не найден — перезапустите программу",
                    "installed but not found — restart the app",
                )
                .to_owned(),
                (Err(err), false) => err,
            };
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn job(id: u64, status: JobStatus, info_ready: bool) -> Job {
        Job {
            id,
            input: PathBuf::from(format!("C:/media/clip{id}.mkv")),
            output: PathBuf::from(format!("C:/media/clip{id}.mp4")),
            status,
            info: MediaInfo { duration: Some(60.0), width: Some(1920), height: Some(1080), ..Default::default() },
            info_ready,
            shake: false,
        }
    }

    /// Один кадр окна 1000×800 в момент `time`; возвращает, через сколько окно попросит следующий кадр.
    fn frame(ctx: &egui::Context, app: &mut App, time: f64) -> Duration {
        let input = egui::RawInput {
            time: Some(time),
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 800.0))),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| crate::ui::draw(app, ui));
        out.textures_delta.clear();
        out.viewport_output.values().map(|v| v.repaint_delay).min().unwrap_or(Duration::MAX)
    }

    fn app_with(ctx: &egui::Context, lang: anvil_ui::Lang) -> App {
        let mut config = AppConfig::default();
        config.common.language = lang;
        config.common.check_updates = false; // тест не ходит в сеть
        let mut app = App::build(ctx, config);
        // Окно уже «показано», подгонять размер не нужно.
        app.startup_size_sent = true;
        app.startup_shown = true;
        app
    }

    /// Все состояния очереди, оба языка, движение включено и выключено, все переключения блоков
    /// настроек и диалоги: рисуется без паники (а в отладочной сборке egui — и без конфликтов Id).
    #[test]
    fn every_state_draws_in_both_languages_with_motion_on_and_off() {
        for lang in [anvil_ui::Lang::Ru, anvil_ui::Lang::En] {
            for reduced in [false, true] {
                let ctx = egui::Context::default();
                let mut app = app_with(&ctx, lang);
                anvil_ui::motion::set_reduced(&ctx, reduced);
                app.jobs = vec![
                    job(1, JobStatus::Pending, false),
                    job(2, JobStatus::Running(40.0), true),
                    job(3, JobStatus::Done, true),
                    job(
                        4,
                        JobStatus::Failed(Failure::Exit(
                            Some(1),
                            "Invalid data
bad input"
                                .into(),
                        )),
                        true,
                    ),
                    job(5, JobStatus::Failed(Failure::Spawn("not found".into())), true),
                ];
                app.jobs[3].shake = true;
                app.jobs[1].info.duration = None; // ход без длительности — неопределённая полоса
                app.processing = true;
                let mut t = 0.0;
                for step in 0..60 {
                    match step {
                        10 => app.settings.video_codec = "copy".into(),
                        20 => app.settings.video_codec = "h264_nvenc".into(),
                        25 => app.settings.audio_codec = "none".into(),
                        30 => app.container_ext = "mkv".into(),
                        35 => {
                            app.show_settings = true;
                            app.show_about = true;
                        }
                        _ => {}
                    }
                    frame(&ctx, &mut app, t);
                    t += 0.05;
                }
            }
        }
    }

    /// В покое (ничего не идёт, все анимации доиграли) окно не просит перерисовок: ни процессора,
    /// ни лишней работы видеокарты. Правило набора: «бесконечное движение — только пока идёт процесс».
    #[test]
    fn idle_window_asks_for_no_frames() {
        let ctx = egui::Context::default();
        let mut app = app_with(&ctx, anvil_ui::Lang::En);
        app.jobs = vec![job(1, JobStatus::Pending, true), job(2, JobStatus::Done, true)];
        let mut t = 0.0;
        for _ in 0..80 {
            frame(&ctx, &mut app, t);
            t += 0.1;
        }
        let delay = frame(&ctx, &mut app, t);
        assert!(delay > Duration::from_secs(1), "в покое кадров не просят, а окно просит через {delay:?}");
    }

    /// Пока идёт работа, живая точка и полоса просят кадры (иначе они замерли бы), а как только
    /// работа кончилась — снова тишина.
    #[test]
    fn work_animates_and_then_settles() {
        let ctx = egui::Context::default();
        let mut app = app_with(&ctx, anvil_ui::Lang::Ru);
        app.jobs = vec![job(1, JobStatus::Running(10.0), true), job(2, JobStatus::Pending, true)];
        app.processing = true;
        let mut t = 0.0;
        for _ in 0..20 {
            frame(&ctx, &mut app, t);
            t += 0.1;
        }
        assert!(frame(&ctx, &mut app, t) < Duration::from_millis(100), "идёт процесс — окно живое");
        app.processing = false;
        app.jobs[0].status = JobStatus::Done;
        app.jobs[1].status = JobStatus::Done;
        for _ in 0..80 {
            t += 0.1;
            frame(&ctx, &mut app, t);
        }
        assert!(frame(&ctx, &mut app, t + 0.1) > Duration::from_secs(1), "работа кончилась — покой");
    }
}
