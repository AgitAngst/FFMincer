use crate::ffmpeg::{self, Event, JobSettings, MediaInfo, QueueItem};
use eframe::egui;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

const VIDEO_CODECS: [&str; 6] = ["libx264", "libx265", "h264_nvenc", "hevc_nvenc", "copy", "none"];
const AUDIO_CODECS: [&str; 6] = ["aac", "libmp3lame", "libopus", "flac", "copy", "none"];
const CONTAINERS: [&str; 8] = ["mp4", "mkv", "mov", "webm", "mp3", "m4a", "opus", "flac"];

#[derive(Debug, Clone)]
enum JobStatus {
    Pending,
    Running(f32),
    Done,
    Failed(String),
}

struct Job {
    id: u64,
    input: PathBuf,
    output: PathBuf,
    status: JobStatus,
    info: MediaInfo,
}

#[derive(Clone, Copy, PartialEq)]
enum Lang {
    Ru,
    En,
}

#[derive(Clone, Copy, PartialEq)]
enum Theme {
    Dark,
    Light,
}


pub struct App {
    jobs: Vec<Job>,
    next_id: u64,

    settings: JobSettings,
    container_ext: String,
    output_dir: Option<PathBuf>,
    current_preset: Option<Preset>,

    config: AppConfig,
    theme: Theme, // разрешённая (фактическая) тема
    startup_size_sent: bool,
    startup_shown: bool,
    startup_target_h: f32,
    startup_frames: u32,

    show_settings: bool,
    settings_pos: Option<[f32; 2]>, // позиция окна настроек (центрируется по главному при открытии)
    autostart_cached: Option<bool>,
    ffmpeg_version: Option<String>, // результат последней проверки (None = не найден/не проверяли)
    ffmpeg_checked: bool,
    installing: bool,
    install_rx: Option<Receiver<Result<String, String>>>,
    install_msg: String,

    processing: bool,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    stop_flag: Arc<AtomicBool>,
    current_child: Arc<Mutex<Option<std::process::Child>>>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let config = load_config();
        let theme = resolve_theme(config.theme);
        apply_theme(&cc.egui_ctx, theme);
        // Если геометрия восстановлена из конфига — окно уже открыто нужного размера,
        // подгонять и заново показывать не нужно.
        let has_geometry = config.geometry.is_some();
        let output_dir = if config.output_dir.trim().is_empty() {
            None
        } else {
            Some(PathBuf::from(&config.output_dir))
        };
        let mut settings = JobSettings::default();
        settings.threads = config.threads;
        settings.keep_metadata = config.keep_metadata;
        let (tx, rx) = std::sync::mpsc::channel();
        Self {
            jobs: Vec::new(),
            next_id: 1,
            settings,
            container_ext: "mp4".into(),
            output_dir,
            current_preset: None,
            config,
            theme,
            startup_size_sent: has_geometry,
            startup_shown: has_geometry,
            startup_target_h: 0.0,
            startup_frames: 0,
            show_settings: false,
            settings_pos: None,
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

    fn add_file(&mut self, path: PathBuf) {
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
        self.jobs.push(Job {
            id: self.next_id,
            input: path,
            output,
            status: JobStatus::Pending,
            info,
        });
        self.next_id += 1;
    }

    fn apply_preset(&mut self, preset: Preset) {
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

    fn recompute_pending_outputs(&mut self) {
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

    fn start_queue(&mut self) {
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

    fn cancel_queue(&mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        if let Ok(mut guard) = self.current_child.lock() {
            if let Some(child) = guard.as_mut() {
                let _ = child.kill();
            }
        }
    }

    fn clear_finished(&mut self) {
        self.jobs.retain(|j| !matches!(j.status, JobStatus::Done));
    }

    fn persist(&self) {
        save_config(&self.config);
    }

    fn set_theme_pref(&mut self, ctx: &egui::Context, pref: ThemePref) {
        self.config.theme = pref;
        self.theme = resolve_theme(pref);
        apply_theme(ctx, self.theme);
        self.persist();
    }

    fn start_ffmpeg_install(&mut self) {
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

    fn settings_window(&mut self, ctx: &egui::Context, lang: Lang) {
        if !self.show_settings {
            self.autostart_cached = None;
            self.settings_pos = None;
            return;
        }

        const SIZE: [f32; 2] = [500.0, 660.0];

        // Позицию считаем один раз при открытии — по центру главного окна.
        if self.settings_pos.is_none() {
            let center = ctx
                .input(|i| i.viewport().outer_rect)
                .map(|r| r.center())
                .or_else(|| self.config.geometry.map(|[w, h, x, y]| egui::pos2(x + w * 0.5, y + h * 0.5)))
                .unwrap_or(egui::pos2(400.0, 300.0));
            self.settings_pos = Some([center.x - SIZE[0] * 0.5, (center.y - SIZE[1] * 0.5).max(0.0)]);
        }

        let mut builder = egui::ViewportBuilder::default()
            .with_title(tip(lang, "Настройки — FFMincer", "Settings — FFMincer"))
            .with_inner_size(SIZE)
            .with_min_inner_size([420.0, 340.0]);
        if let Some(pos) = self.settings_pos {
            builder = builder.with_position(pos);
        }

        // Отдельное системное окно (отдельный viewport), а не встроенное окно внутри главного.
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("ffmincer_settings"),
            builder,
            |vctx, _class| {
                // Окно настроек — в текущей теме приложения.
                apply_theme(vctx, self.theme);

                egui::CentralPanel::default().show(vctx, |ui| {
                    egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
                        let field_w = 260.0;

                        // ===== Приложение =====
                        ui.heading(tip(lang, "Приложение", "Application"));
                        ui.add_space(6.0);
                        egui::Grid::new("g_app")
                            .num_columns(2)
                            .spacing([16.0, 10.0])
                            .min_col_width(130.0)
                            .show(ui, |ui| {
                                ui.label(tip(lang, "Тема", "Theme"));
                                let mut pref = self.config.theme;
                                egui::ComboBox::from_id_source("cfg_theme")
                                    .width(field_w)
                                    .selected_text(theme_pref_label(pref, lang))
                                    .show_ui(ui, |ui| {
                                        for p in [ThemePref::Dark, ThemePref::Light, ThemePref::System] {
                                            ui.selectable_value(&mut pref, p, theme_pref_label(p, lang));
                                        }
                                    });
                                if pref != self.config.theme {
                                    self.set_theme_pref(ctx, pref);
                                }
                                ui.end_row();

                                ui.label(tip(lang, "Язык", "Language"));
                                ui.horizontal(|ui| {
                                    let mut l = self.config.lang;
                                    ui.selectable_value(&mut l, Lang::Ru, "RU");
                                    ui.selectable_value(&mut l, Lang::En, "EN");
                                    if l != self.config.lang {
                                        self.config.lang = l;
                                        self.persist();
                                    }
                                });
                                ui.end_row();

                                ui.label(tip(lang, "По завершении", "When finished"));
                                let mut act = self.config.post_action;
                                egui::ComboBox::from_id_source("cfg_post")
                                    .width(field_w)
                                    .selected_text(post_action_label(act, lang))
                                    .show_ui(ui, |ui| {
                                        for a in [PostAction::None, PostAction::OpenFolder, PostAction::Sleep, PostAction::Shutdown] {
                                            ui.selectable_value(&mut act, a, post_action_label(a, lang));
                                        }
                                    });
                                if act != self.config.post_action {
                                    self.config.post_action = act;
                                    self.persist();
                                }
                                ui.end_row();
                            });

                        ui.add_space(8.0);
                        let mut autostart = *self.autostart_cached.get_or_insert_with(crate::sys::is_autostart_enabled);
                        if ui.checkbox(&mut autostart, tip(lang, "Запускать вместе с системой", "Launch at system startup")).changed() {
                            let ok = crate::sys::set_autostart(autostart);
                            self.autostart_cached = Some(if ok { autostart } else { !autostart });
                        }
                        if ui.checkbox(&mut self.config.autostart_queue, tip(lang, "Начинать конвертацию сразу при добавлении", "Start converting as soon as files are added")).changed() {
                            self.persist();
                        }
                        if ui.checkbox(&mut self.config.autoclear_finished, tip(lang, "Убирать завершённые из списка", "Remove finished from the list")).changed() {
                            self.persist();
                        }
                        if ui.checkbox(&mut self.config.sound_on_finish, tip(lang, "Звук по завершении очереди", "Sound when the queue finishes")).changed() {
                            self.persist();
                        }

                        ui.add_space(12.0);
                        ui.separator();
                        ui.add_space(6.0);

                        // ===== ffmpeg =====
                        ui.heading("ffmpeg");
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            if ui.button(tip(lang, "Проверить", "Check")).clicked() {
                                self.ffmpeg_version = crate::sys::ffmpeg_version(&self.config.ffmpeg_path);
                                self.ffmpeg_checked = true;
                            }
                            match &self.ffmpeg_version {
                                Some(v) => {
                                    ui.colored_label(egui::Color32::from_rgb(120, 200, 140), v);
                                }
                                None => {
                                    let txt = if self.ffmpeg_checked {
                                        tip(lang, "не найден", "not found")
                                    } else {
                                        tip(lang, "не проверялось", "not checked")
                                    };
                                    ui.colored_label(egui::Color32::from_rgb(220, 140, 120), txt);
                                }
                            }
                        });
                        ui.horizontal(|ui| {
                            let btn = egui::Button::new(tip(lang, "Установить тихо (winget)", "Install silently (winget)"));
                            if ui.add_enabled(!self.installing, btn).clicked() {
                                self.start_ffmpeg_install();
                            }
                            if self.installing {
                                ui.spinner();
                                ui.label(tip(lang, "установка…", "installing…"));
                            } else if !self.install_msg.is_empty() {
                                ui.label(&self.install_msg);
                            }
                        });

                        ui.add_space(6.0);
                        egui::Grid::new("g_ffmpeg")
                            .num_columns(2)
                            .spacing([16.0, 10.0])
                            .min_col_width(130.0)
                            .show(ui, |ui| {
                                ui.label(tip(lang, "Путь ffmpeg", "ffmpeg path"));
                                ui.horizontal(|ui| {
                                    if ui.add(egui::TextEdit::singleline(&mut self.config.ffmpeg_path).desired_width(field_w).hint_text("ffmpeg")).lost_focus() {
                                        self.persist();
                                    }
                                    if ui.button("…").clicked() {
                                        if let Some(p) = rfd::FileDialog::new().add_filter("exe", &["exe"]).pick_file() {
                                            self.config.ffmpeg_path = p.display().to_string();
                                            self.persist();
                                        }
                                    }
                                });
                                ui.end_row();

                                ui.label(tip(lang, "Путь ffprobe", "ffprobe path"));
                                ui.horizontal(|ui| {
                                    if ui.add(egui::TextEdit::singleline(&mut self.config.ffprobe_path).desired_width(field_w).hint_text("ffprobe")).lost_focus() {
                                        self.persist();
                                    }
                                    if ui.button("…").clicked() {
                                        if let Some(p) = rfd::FileDialog::new().add_filter("exe", &["exe"]).pick_file() {
                                            self.config.ffprobe_path = p.display().to_string();
                                            self.persist();
                                        }
                                    }
                                });
                                ui.end_row();

                                ui.label(tip(lang, "Потоки (-threads)", "Threads (-threads)"));
                                ui.horizontal(|ui| {
                                    let mut t = self.config.threads as i32;
                                    if ui.add(egui::DragValue::new(&mut t).clamp_range(0..=64)).changed() {
                                        self.config.threads = t.max(0) as u32;
                                        self.settings.threads = self.config.threads;
                                        self.persist();
                                    }
                                    ui.label(egui::RichText::new(tip(lang, "0 = авто", "0 = auto")).weak());
                                });
                                ui.end_row();
                            });
                        ui.add_space(6.0);
                        if ui.checkbox(&mut self.config.low_priority, tip(lang, "Низкий приоритет процесса ffmpeg", "Low ffmpeg process priority")).changed() {
                            self.persist();
                        }

                        ui.add_space(12.0);
                        ui.separator();
                        ui.add_space(6.0);

                        // ===== Вывод =====
                        ui.heading(tip(lang, "Вывод", "Output"));
                        ui.add_space(6.0);
                        egui::Grid::new("g_out")
                            .num_columns(2)
                            .spacing([16.0, 10.0])
                            .min_col_width(130.0)
                            .show(ui, |ui| {
                                ui.label(tip(lang, "Шаблон имени", "Name template"));
                                if ui.add(egui::TextEdit::singleline(&mut self.config.name_template).desired_width(field_w).hint_text("{name}_converted")).lost_focus() {
                                    if self.config.name_template.trim().is_empty() {
                                        self.config.name_template = "{name}".into();
                                    }
                                    self.persist();
                                    self.recompute_pending_outputs();
                                }
                                ui.end_row();

                                ui.label(tip(lang, "При совпадении", "On conflict"));
                                let mut ov = self.config.overwrite;
                                egui::ComboBox::from_id_source("cfg_conflict")
                                    .width(field_w)
                                    .selected_text(if ov {
                                        tip(lang, "перезаписывать", "overwrite")
                                    } else {
                                        tip(lang, "переименовывать", "rename")
                                    })
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(&mut ov, false, tip(lang, "переименовывать", "rename"));
                                        ui.selectable_value(&mut ov, true, tip(lang, "перезаписывать", "overwrite"));
                                    });
                                if ov != self.config.overwrite {
                                    self.config.overwrite = ov;
                                    self.persist();
                                    self.recompute_pending_outputs();
                                }
                                ui.end_row();
                            });
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new(tip(lang, "{name} — имя исходного файла", "{name} — source file name")).weak().small());
                        ui.add_space(6.0);
                        if ui.checkbox(&mut self.config.keep_metadata, tip(lang, "Сохранять метаданные исходника", "Keep source metadata")).changed() {
                            self.settings.keep_metadata = self.config.keep_metadata;
                            self.persist();
                        }
                    });
                });

                // Закрытие окна крестиком ОС.
                if vctx.input(|i| i.viewport().close_requested()) {
                    self.show_settings = false;
                    self.autostart_cached = None;
                }
            },
        );
    }
}

fn theme_pref_label(pref: ThemePref, lang: Lang) -> &'static str {
    match pref {
        ThemePref::Dark => tip(lang, "Тёмная", "Dark"),
        ThemePref::Light => tip(lang, "Светлая", "Light"),
        ThemePref::System => tip(lang, "Системная", "System"),
    }
}

fn post_action_label(action: PostAction, lang: Lang) -> &'static str {
    match action {
        PostAction::None => tip(lang, "Ничего", "Nothing"),
        PostAction::OpenFolder => tip(lang, "Открыть папку вывода", "Open output folder"),
        PostAction::Sleep => tip(lang, "Сон ПК", "Sleep PC"),
        PostAction::Shutdown => tip(lang, "Выключить ПК", "Shut down PC"),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Preset {
    Mp4Cpu,
    Mp4Nvenc,
    Mkv265,
    Mp3,
    Flac,
}

impl Preset {
    const ALL: [Preset; 5] = [
        Preset::Mp4Cpu,
        Preset::Mp4Nvenc,
        Preset::Mkv265,
        Preset::Mp3,
        Preset::Flac,
    ];

    fn label(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Preset::Mp4Cpu, Lang::Ru) => "MP4 (CPU, H.264)",
            (Preset::Mp4Cpu, Lang::En) => "MP4 (CPU, H.264)",
            (Preset::Mp4Nvenc, Lang::Ru) => "MP4 (NVENC, быстро)",
            (Preset::Mp4Nvenc, Lang::En) => "MP4 (NVENC, fast)",
            (Preset::Mkv265, Lang::Ru) => "MKV (H.265)",
            (Preset::Mkv265, Lang::En) => "MKV (H.265)",
            (Preset::Mp3, Lang::Ru) => "MP3 (аудио)",
            (Preset::Mp3, Lang::En) => "MP3 (audio)",
            (Preset::Flac, Lang::Ru) => "FLAC (без потерь)",
            (Preset::Flac, Lang::En) => "FLAC (lossless)",
        }
    }
}

/// Набор всех локализуемых строк интерфейса.
struct Tr {
    subtitle: &'static str,
    settings: &'static str,
    presets: &'static str,
    preset_pick: &'static str,
    video_codec: &'static str,
    hw_decode: &'static str,
    resolution: &'static str,
    resolution_hint: &'static str,
    preset_label: &'static str,
    video_bitrate: &'static str,
    video_bitrate_hint: &'static str,
    crf: &'static str,
    audio_codec: &'static str,
    audio_bitrate: &'static str,
    container: &'static str,
    output_folder: &'static str,
    same_as_source: &'static str,
    choose_folder: &'static str,
    reset: &'static str,
    start: &'static str,
    cancel: &'static str,
    clear_finished: &'static str,
    queued: &'static str,
    add_files: &'static str,
    drop_hint: &'static str,
    media_filter: &'static str,
    all_files: &'static str,
    queue_empty: &'static str,
    queue_empty_hint: &'static str,
    pending: &'static str,
    done: &'static str,
    error: &'static str,
    remove_from_queue: &'static str,
    theme_tooltip: &'static str,
    lang_tooltip: &'static str,
    overall: &'static str,
    custom_bitrate: &'static str,
    loudnorm: &'static str,
    faststart: &'static str,
}

const RU: Tr = Tr {
    subtitle: "конвертация аудио и видео через ffmpeg",
    settings: "Настройки",
    presets: "Быстрые пресеты:",
    preset_pick: "— выбрать —",
    video_codec: "Видеокодек",
    hw_decode: "Аппаратное декодирование (CUDA)",
    resolution: "Разрешение:",
    resolution_hint: "(например 1920x1080, пусто = как есть)",
    preset_label: "Preset:",
    video_bitrate: "Видеобитрейт:",
    video_bitrate_hint: "напр. 8M (пусто = CRF)",
    crf: "CRF (меньше = лучше)",
    audio_codec: "Аудиокодек",
    audio_bitrate: "Аудиобитрейт:",
    container: "Контейнер (расширение)",
    output_folder: "Папка вывода:",
    same_as_source: "как у исходника",
    choose_folder: "Выбрать папку…",
    reset: "Сбросить",
    start: "▶ Начать конвертацию",
    cancel: "⏹ Отменить",
    clear_finished: "Очистить завершённые",
    queued: "В очереди: ",
    add_files: "➕ Добавить файлы…",
    drop_hint: "или перетащите файлы сюда",
    media_filter: "Медиа",
    all_files: "Все файлы",
    queue_empty: "📂 Очередь пуста",
    queue_empty_hint: "Перетащите видео или аудио файлы в это окно",
    pending: "Ожидание",
    done: "✔ Готово",
    error: "Ошибка",
    remove_from_queue: "Убрать из очереди",
    theme_tooltip: "Переключить тему",
    lang_tooltip: "Сменить язык",
    overall: "Общий прогресс:",
    custom_bitrate: "своё:",
    loudnorm: "Нормализовать громкость",
    faststart: "Быстрый старт (faststart)",
};

const EN: Tr = Tr {
    subtitle: "audio & video conversion via ffmpeg",
    settings: "Settings",
    presets: "Quick presets:",
    preset_pick: "— pick —",
    video_codec: "Video codec",
    hw_decode: "Hardware decoding (CUDA)",
    resolution: "Resolution:",
    resolution_hint: "(e.g. 1920x1080, empty = source)",
    preset_label: "Preset:",
    video_bitrate: "Video bitrate:",
    video_bitrate_hint: "e.g. 8M (empty = CRF)",
    crf: "CRF (lower = better)",
    audio_codec: "Audio codec",
    audio_bitrate: "Audio bitrate:",
    container: "Container (extension)",
    output_folder: "Output folder:",
    same_as_source: "same as source",
    choose_folder: "Choose folder…",
    reset: "Reset",
    start: "▶ Start conversion",
    cancel: "⏹ Cancel",
    clear_finished: "Clear finished",
    queued: "Queued: ",
    add_files: "➕ Add files…",
    drop_hint: "or drop files here",
    media_filter: "Media",
    all_files: "All files",
    queue_empty: "📂 Queue is empty",
    queue_empty_hint: "Drop video or audio files onto this window",
    pending: "Pending",
    done: "✔ Done",
    error: "Error",
    remove_from_queue: "Remove from queue",
    theme_tooltip: "Toggle theme",
    lang_tooltip: "Change language",
    overall: "Overall progress:",
    custom_bitrate: "custom:",
    loudnorm: "Normalize loudness",
    faststart: "Fast start (faststart)",
};

fn strings(lang: Lang) -> &'static Tr {
    match lang {
        Lang::Ru => &RU,
        Lang::En => &EN,
    }
}

/// Путь к файлу с сохранёнными настройками (`%APPDATA%\FFMincer\config.txt`).
fn config_path() -> Option<PathBuf> {
    let mut dir = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))?;
    dir.push("FFMincer");
    Some(dir.join("config.txt"))
}

#[derive(Clone, Copy, PartialEq)]
enum ThemePref {
    Dark,
    Light,
    System,
}

#[derive(Clone, Copy, PartialEq)]
enum PostAction {
    None,
    OpenFolder,
    Sleep,
    Shutdown,
}

/// Все сохраняемые настройки приложения (файл `%APPDATA%\FFMincer\config.txt`).
#[derive(Clone)]
struct AppConfig {
    lang: Lang,
    theme: ThemePref,
    geometry: Option<[f32; 4]>,
    autostart_queue: bool,
    autoclear_finished: bool,
    sound_on_finish: bool,
    post_action: PostAction,
    ffmpeg_path: String,
    ffprobe_path: String,
    low_priority: bool,
    threads: u32,
    output_dir: String,   // "" = рядом с исходником
    name_template: String,
    overwrite: bool,      // true = перезаписывать, false = переименовывать
    keep_metadata: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            lang: Lang::Ru,
            theme: ThemePref::Dark,
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

fn load_config() -> AppConfig {
    let mut c = AppConfig::default();
    let (mut w, mut h, mut x, mut y) = (None, None, None, None);
    if let Some(path) = config_path() {
        if let Ok(text) = std::fs::read_to_string(&path) {
            for line in text.lines() {
                let Some((key, value)) = line.split_once('=') else {
                    continue;
                };
                let (key, value) = (key.trim(), value.trim());
                match key {
                    "lang" => c.lang = if value == "en" { Lang::En } else { Lang::Ru },
                    "theme" => {
                        c.theme = match value {
                            "light" => ThemePref::Light,
                            "system" => ThemePref::System,
                            _ => ThemePref::Dark,
                        }
                    }
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
    load_config().geometry
}

fn save_config(c: &AppConfig) {
    let Some(path) = config_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let lang = match c.lang {
        Lang::Ru => "ru",
        Lang::En => "en",
    };
    let theme = match c.theme {
        ThemePref::Dark => "dark",
        ThemePref::Light => "light",
        ThemePref::System => "system",
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
        text.push_str(&format!(
            "win_w={:.0}\nwin_h={:.0}\nwin_x={:.0}\nwin_y={:.0}\n",
            w, h, x, y
        ));
    }
    let _ = std::fs::write(&path, text);
}

fn resolve_theme(pref: ThemePref) -> Theme {
    match pref {
        ThemePref::Dark => Theme::Dark,
        ThemePref::Light => Theme::Light,
        ThemePref::System => {
            if crate::sys::system_is_dark() {
                Theme::Dark
            } else {
                Theme::Light
            }
        }
    }
}

fn apply_theme(ctx: &egui::Context, theme: Theme) {
    let mut visuals = match theme {
        Theme::Dark => egui::Visuals::dark(),
        Theme::Light => egui::Visuals::light(),
    };
    let rounding = egui::Rounding::same(8.0);
    visuals.window_rounding = egui::Rounding::same(12.0);
    visuals.widgets.noninteractive.rounding = rounding;
    visuals.widgets.inactive.rounding = rounding;
    visuals.widgets.hovered.rounding = rounding;
    visuals.widgets.active.rounding = rounding;

    let accent = egui::Color32::from_rgb(88, 166, 255);
    visuals.selection.bg_fill = accent;
    visuals.hyperlink_color = accent;

    if matches!(theme, Theme::Dark) {
        visuals.panel_fill = egui::Color32::from_rgb(24, 26, 32);
        visuals.window_fill = egui::Color32::from_rgb(24, 26, 32);
    }
    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.spacing.button_padding = egui::vec2(14.0, 7.0);
    ctx.set_style(style);
}

#[derive(Clone, Copy)]
enum BitrateKind {
    Audio,
    Video,
}

/// Частые значения битрейта в кбит/с.
const AUDIO_BITRATES: [u32; 6] = [96, 128, 160, 192, 256, 320];
const VIDEO_BITRATES: [u32; 9] = [1000, 2000, 4000, 6000, 8000, 12000, 16000, 25000, 50000];

/// Разбирает строку битрейта ("192k", "8M", "8000") в кбит/с.
fn parse_kbps(s: &str) -> Option<u32> {
    let s = s.trim().to_ascii_lowercase();
    if s.is_empty() {
        return None;
    }
    if let Some(num) = s.strip_suffix('k') {
        num.trim().parse::<f64>().ok().map(|v| v.round() as u32)
    } else if let Some(num) = s.strip_suffix('m') {
        num.trim().parse::<f64>().ok().map(|v| (v * 1000.0).round() as u32)
    } else {
        s.parse::<f64>().ok().map(|v| v.round() as u32)
    }
}

fn nearest_index(options: &[u32], target: u32) -> usize {
    options
        .iter()
        .enumerate()
        .min_by_key(|(_, v)| (**v as i64 - target as i64).abs())
        .map(|(i, _)| i)
        .unwrap_or(0)
}

fn fmt_duration(secs: f64) -> String {
    let total = secs.max(0.0) as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn fmt_size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.2} GB", b / (1024.0 * 1024.0 * 1024.0))
    } else if b >= 1024.0 * 1024.0 {
        format!("{:.1} MB", b / (1024.0 * 1024.0))
    } else {
        format!("{:.0} KB", b / 1024.0)
    }
}

/// Собирает строку вида "1920×1080 · h264/aac · 12:34 · 45.2 MB". Пусто, если нечего показать.
fn format_media_info(info: &MediaInfo) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let (Some(w), Some(h)) = (info.width, info.height) {
        parts.push(format!("{w}×{h}"));
    }
    let codecs: Vec<&str> = [info.v_codec.as_deref(), info.a_codec.as_deref()]
        .into_iter()
        .flatten()
        .collect();
    if !codecs.is_empty() {
        parts.push(codecs.join("/"));
    }
    if let Some(d) = info.duration {
        parts.push(fmt_duration(d));
    }
    if let Some(size) = info.size_bytes {
        parts.push(fmt_size(size));
    }
    parts.join(" · ")
}

fn format_bitrate(kbps: u32, kind: BitrateKind) -> String {
    match kind {
        BitrateKind::Audio => format!("{kbps} kbps"),
        BitrateKind::Video => {
            if kbps % 1000 == 0 {
                format!("{} Mbps", kbps / 1000)
            } else {
                format!("{:.1} Mbps", kbps as f64 / 1000.0)
            }
        }
    }
}

/// Выбирает подсказку по текущему языку.
fn tip(lang: Lang, ru: &'static str, en: &'static str) -> &'static str {
    match lang {
        Lang::Ru => ru,
        Lang::En => en,
    }
}

fn tip_video_codec(code: &str, lang: Lang) -> &'static str {
    match code {
        "libx264" => tip(
            lang,
            "H.264 на CPU. Универсально, отличная совместимость.\nХорошо: CRF 18–23, preset medium/slow.\nПлохо: preset=veryslow ради 1% размера; очень долго на 4K.",
            "H.264 on CPU. Universal, great compatibility.\nGood: CRF 18–23, preset medium/slow.\nBad: preset=veryslow for a 1% gain; very slow on 4K.",
        ),
        "libx265" => tip(
            lang,
            "H.265/HEVC на CPU. ~30% меньше размер при том же качестве, но медленнее.\nХорошо: архив 4K, CRF 20–26.\nПлохо: старые устройства/браузеры могут не открыть.",
            "H.265/HEVC on CPU. ~30% smaller at same quality, but slower.\nGood: 4K archiving, CRF 20–26.\nBad: old devices/browsers may not play it.",
        ),
        "h264_nvenc" => tip(
            lang,
            "H.264 на видеокарте NVIDIA. Очень быстро.\nХорошо: битрейт 8–16M для 1080p.\nПлохо: CRF не работает — задавай битрейт; 2–3M → артефакты.",
            "H.264 on an NVIDIA GPU. Very fast.\nGood: 8–16M bitrate for 1080p.\nBad: CRF has no effect — set a bitrate; 2–3M → artifacts.",
        ),
        "hevc_nvenc" => tip(
            lang,
            "H.265 на видеокарте NVIDIA. Быстро и компактнее H.264.\nХорошо: 6–10M для 1080p, 20–40M для 4K.\nПлохо: слишком низкий битрейт → мыло.",
            "H.265 on an NVIDIA GPU. Fast and smaller than H.264.\nGood: 6–10M for 1080p, 20–40M for 4K.\nBad: too low a bitrate → mushy image.",
        ),
        "copy" => tip(
            lang,
            "Видеопоток копируется без перекодирования. Мгновенно, без потерь.\nХорошо: сменить контейнер (mkv→mp4).\nПлохо: контейнер не поддерживает кодек; нельзя менять разрешение/битрейт.",
            "Video stream copied without re-encoding. Instant, lossless.\nGood: change container (mkv→mp4).\nBad: container can't hold the codec; can't change resolution/bitrate.",
        ),
        "none" => tip(
            lang,
            "Без видео — на выходе только звук.\nХорошо: извлечь аудио в mp3/flac.\nПлохо: выбрать для клипа, который нужен с картинкой.",
            "No video — audio-only output.\nGood: extract audio to mp3/flac.\nBad: choosing it for a clip you want to keep the picture.",
        ),
        _ => "",
    }
}

fn tip_audio_codec(code: &str, lang: Lang) -> &'static str {
    match code {
        "aac" => tip(
            lang,
            "Стандарт для mp4/mkv, хорошая совместимость.\nХорошо: 128–256k.\nПлохо: ниже 96k — заметное падение качества.",
            "The standard for mp4/mkv, good compatibility.\nGood: 128–256k.\nBad: below 96k — audible quality loss.",
        ),
        "libmp3lame" => tip(
            lang,
            "MP3, играется везде.\nХорошо: 192–320k.\nПлохо: 64–96k для музыки — глухо и с артефактами.",
            "MP3, plays everywhere.\nGood: 192–320k.\nBad: 64–96k for music — dull and artifacty.",
        ),
        "libopus" => tip(
            lang,
            "Opus — лучший звук на низком битрейте.\nХорошо: 96–160k ≈ mp3 320k.\nПлохо: старые плееры могут не поддержать; в mp4 кладётся плохо.",
            "Opus — best sound at low bitrate.\nGood: 96–160k ≈ mp3 320k.\nBad: old players may not support it; awkward inside mp4.",
        ),
        "flac" => tip(
            lang,
            "FLAC — без потерь.\nХорошо: архив, мастеринг.\nПлохо: битрейт игнорируется, файлы большие; не для стриминга.",
            "FLAC — lossless.\nGood: archiving, mastering.\nBad: bitrate is ignored, big files; not for streaming.",
        ),
        "copy" => tip(
            lang,
            "Звук копируется как есть — без потерь и мгновенно.\nХорошо: смена контейнера.\nПлохо: контейнер не поддерживает исходный кодек.",
            "Audio copied as-is — lossless and instant.\nGood: changing container.\nBad: container doesn't support the source codec.",
        ),
        "none" => tip(
            lang,
            "Без звука — только видео.\nХорошо: немой клип, исходник для гифки.\nПлохо: случайно потерять звуковую дорожку.",
            "No audio — video only.\nGood: silent clip, source for a GIF.\nBad: accidentally dropping the audio track.",
        ),
        _ => "",
    }
}

fn tip_container(ext: &str, lang: Lang) -> &'static str {
    match ext {
        "mp4" => tip(
            lang,
            "Максимальная совместимость (H.264/H.265 + AAC).\nХорошо: публикация, телефоны, браузеры.\nПлохо: opus/vorbis, много субтитров и дорожек.",
            "Maximum compatibility (H.264/H.265 + AAC).\nGood: sharing, phones, browsers.\nBad: opus/vorbis, many subtitle/audio tracks.",
        ),
        "mkv" => tip(
            lang,
            "Всеядный контейнер: любые кодеки, дорожки, субтитры.\nХорошо: архив, H.265, несколько аудио.\nПлохо: хуже поддержка в браузерах и на ТВ.",
            "Takes anything: any codec, tracks, subtitles.\nGood: archiving, H.265, multiple audio.\nBad: weaker support in browsers and on TVs.",
        ),
        "mov" => tip(
            lang,
            "Мир Apple/ProRes.\nХорошо: монтаж в Final Cut/Premiere.\nПлохо: избыточно для обычной раздачи.",
            "Apple/ProRes world.\nGood: editing in Final Cut/Premiere.\nBad: overkill for plain sharing.",
        ),
        "webm" => tip(
            lang,
            "Для веба (VP9/AV1 + Opus/Vorbis).\nХорошо: сайты, HTML5-video.\nПлохо: H.264/AAC сюда не положить — кодеки не те.",
            "For the web (VP9/AV1 + Opus/Vorbis).\nGood: websites, HTML5 video.\nBad: can't hold H.264/AAC — wrong codecs.",
        ),
        "mp3" => tip(
            lang,
            "Только звук, MP3.\nХорошо: музыка/подкаст, играется везде.\nПлохо: видео потеряется, без потерь не будет.",
            "Audio only, MP3.\nGood: music/podcast, plays everywhere.\nBad: video is lost, not lossless.",
        ),
        "m4a" => tip(
            lang,
            "Аудио-контейнер для AAC/ALAC.\nХорошо: AAC-звук для Apple и телефонов.\nПлохо: не для mp3-потока.",
            "Audio container for AAC/ALAC.\nGood: AAC audio for Apple and phones.\nBad: not for an MP3 stream.",
        ),
        "opus" => tip(
            lang,
            "Контейнер .opus только для Opus.\nХорошо: компактный голос/музыка.\nПлохо: слабая поддержка плеерами.",
            ".opus container, Opus only.\nGood: compact voice/music.\nBad: weak player support.",
        ),
        "flac" => tip(
            lang,
            "Только FLAC, без потерь.\nХорошо: архив музыки.\nПлохо: большие файлы, не для стриминга.",
            "FLAC only, lossless.\nGood: music archiving.\nBad: big files, not for streaming.",
        ),
        _ => "",
    }
}

fn tip_preset(preset: Preset, lang: Lang) -> &'static str {
    match preset {
        Preset::Mp4Cpu => tip(
            lang,
            "libx264 + AAC, CRF 23, mp4. Баланс качества и совместимости.\nХорошо: почти всё.\nПлохо: медленно на слабом CPU.",
            "libx264 + AAC, CRF 23, mp4. Balanced quality/compatibility.\nGood: almost everything.\nBad: slow on a weak CPU.",
        ),
        Preset::Mp4Nvenc => tip(
            lang,
            "h264_nvenc + AAC, 8M, mp4, CUDA. Очень быстро на NVIDIA.\nХорошо: массовая перекодировка.\nПлохо: без видеокарты NVIDIA не заработает.",
            "h264_nvenc + AAC, 8M, mp4, CUDA. Very fast on NVIDIA.\nGood: bulk transcoding.\nBad: won't run without an NVIDIA GPU.",
        ),
        Preset::Mkv265 => tip(
            lang,
            "libx265 + AAC, CRF 24, mkv. Меньше размер.\nХорошо: архив 4K.\nПлохо: медленно, хуже совместимость.",
            "libx265 + AAC, CRF 24, mkv. Smaller size.\nGood: 4K archiving.\nBad: slow, weaker compatibility.",
        ),
        Preset::Mp3 => tip(
            lang,
            "Извлечь звук в MP3 256k.\nХорошо: музыка из видео.\nПлохо: видео отбрасывается.",
            "Extract audio to MP3 256k.\nGood: music out of a video.\nBad: the video is discarded.",
        ),
        Preset::Flac => tip(
            lang,
            "Извлечь звук в FLAC без потерь.\nХорошо: архив.\nПлохо: большой размер.",
            "Extract audio to lossless FLAC.\nGood: archiving.\nBad: large file.",
        ),
    }
}

/// Единый виджет битрейта: ползунок с частыми значениями + поле для своего значения.
fn bitrate_control(ui: &mut egui::Ui, tr: &Tr, lang: Lang, label: &str, value: &mut String, kind: BitrateKind) {
    let options: &[u32] = match kind {
        BitrateKind::Audio => &AUDIO_BITRATES,
        BitrateKind::Video => &VIDEO_BITRATES,
    };

    let slider_tip = match kind {
        BitrateKind::Video => tip(
            lang,
            "Целевой битрейт видео (постоянное качество на GPU/при заданном битрейте).\nХорошо: 8M для 1080p, 20–40M для 4K.\nПлохо: 1M на 1080p — блоки. Пусто при CPU-кодеке = режим CRF.",
            "Target video bitrate (used by GPU encoders or when a bitrate is set).\nGood: 8M for 1080p, 20–40M for 4K.\nBad: 1M at 1080p — blocky. Empty with a CPU codec = CRF mode.",
        ),
        BitrateKind::Audio => tip(
            lang,
            "Битрейт звука.\nХорошо: 192–256k (aac/mp3), 128k (opus).\nПлохо: 64k для музыки; для FLAC значение игнорируется.",
            "Audio bitrate.\nGood: 192–256k (aac/mp3), 128k (opus).\nBad: 64k for music; ignored for FLAC.",
        ),
    };
    let custom_tip = tip(
        lang,
        "Своё значение: 320k, 8M или 8000 (кбит/с). Ползунок встанет на ближайшее частое.",
        "Custom value: 320k, 8M or 8000 (kbps). The slider snaps to the nearest common one.",
    );

    ui.label(label);

    let mut idx = parse_kbps(value)
        .map(|c| nearest_index(options, c))
        .unwrap_or(options.len() / 2);
    let last = options.len() - 1;
    let opts: Vec<u32> = options.to_vec();

    let resp = ui
        .add(
            egui::Slider::new(&mut idx, 0..=last)
                .custom_formatter(move |n, _| {
                    let i = (n.round() as usize).min(opts.len() - 1);
                    format_bitrate(opts[i], kind)
                })
                .custom_parser(|_| None),
        )
        .on_hover_text(slider_tip);
    if resp.changed() {
        *value = format!("{}k", options[idx]);
    }

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(tr.custom_bitrate).weak().small());
        ui.add(egui::TextEdit::singleline(value).desired_width(f32::INFINITY))
            .on_hover_text(custom_tip);
    });
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let tr = strings(self.config.lang);
        let lang = self.config.lang;

        // Результат фоновой установки ffmpeg.
        if let Some(rx) = &self.install_rx {
            if let Ok(res) = rx.try_recv() {
                self.installing = false;
                self.install_rx = None;
                match res {
                    Ok(msg) => self.install_msg = msg,
                    Err(err) => self.install_msg = err,
                }
                self.ffmpeg_version = crate::sys::ffmpeg_version(&self.config.ffmpeg_path);
                self.ffmpeg_checked = true;
            }
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
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        let any_dropped = !dropped.is_empty();
        for path in dropped {
            self.add_file(path);
        }
        if any_dropped && self.config.autostart_queue && !self.processing {
            self.start_queue();
        }
        let hovering_files = ctx.input(|i| !i.raw.hovered_files.is_empty());

        let header_resp = egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading("🥩 FFMincer");
                ui.add(egui::Label::new(egui::RichText::new(tr.subtitle).weak()).truncate(true));

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Кнопка настроек.
                    if ui.button("⚙").on_hover_text(tip(lang, "Настройки", "Settings")).clicked() {
                        self.show_settings = !self.show_settings;
                    }

                    // Переключатель темы (тёмная ↔ светлая; «системная» выбирается в настройках).
                    let (theme_icon, next_pref) = match self.theme {
                        Theme::Dark => ("☀", ThemePref::Light),
                        Theme::Light => ("🌙", ThemePref::Dark),
                    };
                    if ui.button(theme_icon).on_hover_text(tr.theme_tooltip).clicked() {
                        self.set_theme_pref(ctx, next_pref);
                    }

                    // Переключатель языка.
                    ui.scope(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        if ui
                            .selectable_label(self.config.lang == Lang::En, "EN")
                            .on_hover_text(tr.lang_tooltip)
                            .clicked()
                        {
                            self.config.lang = Lang::En;
                            self.persist();
                        }
                        if ui
                            .selectable_label(self.config.lang == Lang::Ru, "RU")
                            .on_hover_text(tr.lang_tooltip)
                            .clicked()
                        {
                            self.config.lang = Lang::Ru;
                            self.persist();
                        }
                    });
                });
            });
            ui.add_space(6.0);
        });

        let mut settings_content_h = 0.0f32;
        egui::SidePanel::right("settings")
            .resizable(true)
            .default_width(300.0)
            .width_range(240.0..=460.0)
            .show(ctx, |ui| {
            ui.add_space(8.0);
            ui.heading(tr.settings);
            ui.add_space(4.0);

            let scroll_out = egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
            ui.label(tr.presets).on_hover_text(tip(
                lang,
                "Готовые наборы настроек. Выбор пункта заполняет кодеки, битрейт и контейнер за тебя. Наведи на пункт списка — там описание.",
                "Ready-made setting bundles. Picking one fills in codecs, bitrate and container for you. Hover a list item for its description.",
            ));
            let mut chosen: Option<Preset> = None;
            egui::ComboBox::from_id_source("preset_combo")
                .width(ui.available_width())
                .selected_text(match self.current_preset {
                    Some(p) => p.label(lang),
                    None => tr.preset_pick,
                })
                .show_ui(ui, |ui| {
                    for p in Preset::ALL {
                        if ui
                            .selectable_label(self.current_preset == Some(p), p.label(lang))
                            .on_hover_text(tip_preset(p, lang))
                            .clicked()
                        {
                            chosen = Some(p);
                        }
                    }
                });
            if let Some(p) = chosen {
                self.current_preset = Some(p);
                self.apply_preset(p);
            }

            ui.separator();

            egui::ComboBox::from_label(tr.video_codec)
                .selected_text(self.settings.video_codec.clone())
                .show_ui(ui, |ui| {
                    for c in VIDEO_CODECS {
                        ui.selectable_value(&mut self.settings.video_codec, c.to_string(), c)
                            .on_hover_text(tip_video_codec(c, lang));
                    }
                })
                .response
                .on_hover_text(tip(
                    lang,
                    "Чем кодируется видео. libx* (CPU) дают качество по CRF; *_nvenc — скорость на GPU по битрейту; copy — без перекодирования; none — убрать видео.",
                    "How video is encoded. libx* (CPU) give CRF-based quality; *_nvenc — GPU speed by bitrate; copy — no re-encode; none — drop video.",
                ));

            let video_active = !matches!(self.settings.video_codec.as_str(), "none" | "copy");
            let is_nvenc = self.settings.video_codec.ends_with("_nvenc");

            if video_active {
                ui.checkbox(&mut self.settings.hw_decode, tr.hw_decode).on_hover_text(tip(
                    lang,
                    "Декодировать вход на GPU (CUDA).\nХорошо: ускоряет NVENC на тяжёлых исходниках.\nПлохо: с редкими кодеками/фильтрами может дать ошибку — тогда выключи.",
                    "Decode input on the GPU (CUDA).\nGood: speeds up NVENC on heavy sources.\nBad: can fail with exotic codecs/filters — turn it off then.",
                ));

                let res_tip = tip(
                    lang,
                    "Масштаб выхода ШИРИНАxВЫСОТА.\nХорошо: 1920x1080, 1280x720.\nПлохо: нечётная высота (1920x1081) ломает H.264; апскейл 720p→4K без смысла. Пусто = как в исходнике.",
                    "Output scale WIDTHxHEIGHT.\nGood: 1920x1080, 1280x720.\nBad: odd height (1920x1081) breaks H.264; upscaling 720p→4K is pointless. Empty = same as source.",
                );
                ui.label(tr.resolution).on_hover_text(res_tip);
                ui.add(egui::TextEdit::singleline(&mut self.settings.resolution).desired_width(f32::INFINITY))
                    .on_hover_text(res_tip);
                ui.label(egui::RichText::new(tr.resolution_hint).weak().small());

                let preset_tip = tip(
                    lang,
                    "Скорость ↔ сжатие.\nlibx264/5: ultrafast..veryslow (медленнее = меньше файл). NVENC: p1..p7.\nХорошо: medium/slow, p5.\nПлохо: placebo/veryslow ради 1% размера.",
                    "Speed ↔ compression.\nlibx264/5: ultrafast..veryslow (slower = smaller file). NVENC: p1..p7.\nGood: medium/slow, p5.\nBad: placebo/veryslow for a 1% gain.",
                );
                ui.label(tr.preset_label).on_hover_text(preset_tip);
                ui.add(egui::TextEdit::singleline(&mut self.settings.preset).desired_width(f32::INFINITY))
                    .on_hover_text(preset_tip);
                ui.label(egui::RichText::new(if is_nvenc { "p1..p7" } else { "ultrafast..veryslow" }).weak().small());

                bitrate_control(ui, tr, lang, tr.video_bitrate, &mut self.settings.video_bitrate, BitrateKind::Video);
                ui.label(egui::RichText::new(tr.video_bitrate_hint).weak().small());

                if self.settings.video_bitrate.trim().is_empty() && !is_nvenc {
                    ui.add(egui::Slider::new(&mut self.settings.crf, 0..=51).text(tr.crf)).on_hover_text(tip(
                        lang,
                        "Постоянное качество для CPU-кодеков. Меньше = лучше и больше файл.\nХорошо: 18–23 (x264), 20–26 (x265).\nПлохо: 0 (огромный файл), 35+ (мыло).",
                        "Constant quality for CPU codecs. Lower = better and bigger file.\nGood: 18–23 (x264), 20–26 (x265).\nBad: 0 (huge file), 35+ (mushy).",
                    ));
                }
            }

            ui.separator();

            egui::ComboBox::from_label(tr.audio_codec)
                .selected_text(self.settings.audio_codec.clone())
                .show_ui(ui, |ui| {
                    for c in AUDIO_CODECS {
                        ui.selectable_value(&mut self.settings.audio_codec, c.to_string(), c)
                            .on_hover_text(tip_audio_codec(c, lang));
                    }
                })
                .response
                .on_hover_text(tip(
                    lang,
                    "Чем кодируется звук. aac/mp3 — совместимость; opus — лучший звук на низком битрейте; flac — без потерь; copy — без перекодирования; none — убрать звук.",
                    "How audio is encoded. aac/mp3 — compatibility; opus — best sound at low bitrate; flac — lossless; copy — no re-encode; none — drop audio.",
                ));

            if !matches!(self.settings.audio_codec.as_str(), "none" | "copy") {
                bitrate_control(ui, tr, lang, tr.audio_bitrate, &mut self.settings.audio_bitrate, BitrateKind::Audio);
                ui.checkbox(&mut self.settings.loudnorm, tr.loudnorm).on_hover_text(tip(
                    lang,
                    "Приводит громкость к стандарту вещания EBU R128 (-af loudnorm).\nХорошо: разнобой по громкости в подборке роликов/музыки.\nПлохо: уже смастеренный трек — можно испортить динамику. Не работает с кодеком copy.",
                    "Brings loudness to the EBU R128 broadcast standard (-af loudnorm).\nGood: a batch of clips/music with uneven volume.\nBad: an already-mastered track — may squash dynamics. Doesn't work with the copy codec.",
                ));
            }

            ui.separator();

            egui::ComboBox::from_label(tr.container)
                .selected_text(self.container_ext.clone())
                .show_ui(ui, |ui| {
                    for c in CONTAINERS {
                        if ui
                            .selectable_value(&mut self.container_ext, c.to_string(), c)
                            .on_hover_text(tip_container(c, lang))
                            .changed()
                        {
                            self.recompute_pending_outputs();
                        }
                    }
                })
                .response
                .on_hover_text(tip(
                    lang,
                    "Расширение/формат выходного файла — определяет, какие кодеки в него влезут.\nХорошо: mp4 для раздачи, mkv для архива.\nПлохо: webm с H.264 (кодеки не те).",
                    "Output file extension/format — decides which codecs fit inside.\nGood: mp4 for sharing, mkv for archiving.\nBad: webm with H.264 (wrong codecs).",
                ));

            if matches!(self.container_ext.as_str(), "mp4" | "mov" | "m4a") {
                ui.checkbox(&mut self.settings.faststart, tr.faststart).on_hover_text(tip(
                    lang,
                    "Переносит индекс файла (moov atom) в начало — видео начинает играть до полной загрузки (-movflags +faststart).\nХорошо: mp4 для сайта/стриминга.\nПлохо: смысла нет для локальных файлов, но и не вредит.",
                    "Moves the file index (moov atom) to the front — video starts playing before it's fully downloaded (-movflags +faststart).\nGood: mp4 for a website/streaming.\nBad: pointless for local-only files, but harmless.",
                ));
            }

            let folder_tip = tip(
                lang,
                "Куда сохранять результат. По умолчанию — рядом с исходником. Исходный файл никогда не перезаписывается.",
                "Where to save the result. Defaults to next to the source. The source file is never overwritten.",
            );
            ui.label(tr.output_folder).on_hover_text(folder_tip);
            let out_dir_text = match &self.output_dir {
                Some(dir) => dir.to_string_lossy().into_owned(),
                None => tr.same_as_source.to_string(),
            };
            ui.add(egui::Label::new(egui::RichText::new(out_dir_text).weak()).truncate(true))
                .on_hover_text(folder_tip);
            ui.horizontal(|ui| {
                if ui.button(tr.choose_folder).clicked() {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        self.config.output_dir = dir.display().to_string();
                        self.output_dir = Some(dir);
                        self.recompute_pending_outputs();
                        self.persist();
                    }
                }
                if self.output_dir.is_some() && ui.button(tr.reset).clicked() {
                    self.output_dir = None;
                    self.config.output_dir.clear();
                    self.recompute_pending_outputs();
                    self.persist();
                }
            });
            }); // ScrollArea
            settings_content_h = scroll_out.content_size.y;
        });

        let actions_resp = egui::TopBottomPanel::bottom("actions").show(ctx, |ui| {
            ui.add_space(6.0);

            // Общий прогресс по всей очереди.
            let total = self.jobs.len();
            let mut progress_sum = 0.0f32;
            for job in &self.jobs {
                match &job.status {
                    JobStatus::Done | JobStatus::Failed(_) => progress_sum += 1.0,
                    JobStatus::Running(pct) => progress_sum += pct / 100.0,
                    JobStatus::Pending => {}
                }
            }
            if total > 0 && (self.processing || progress_sum > 0.0) {
                let overall = progress_sum / total as f32;
                ui.horizontal(|ui| {
                    ui.label(tr.overall);
                    ui.add(egui::ProgressBar::new(overall).show_percentage());
                });
            }

            ui.horizontal(|ui| {
                let pending_count = self.jobs.iter().filter(|j| matches!(j.status, JobStatus::Pending)).count();

                if ui
                    .add_enabled(!self.processing && pending_count > 0, egui::Button::new(tr.start))
                    .clicked()
                {
                    self.start_queue();
                }

                if ui.add_enabled(self.processing, egui::Button::new(tr.cancel)).clicked() {
                    self.cancel_queue();
                }

                if ui.button(tr.clear_finished).clicked() {
                    self.clear_finished();
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("{}{}", tr.queued, pending_count));
                });
            });
            ui.add_space(6.0);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button(tr.add_files).clicked() {
                    if let Some(paths) = rfd::FileDialog::new()
                        .add_filter(
                            tr.media_filter,
                            &[
                                "mp4", "mkv", "mov", "avi", "webm", "flv", "m4v", "wmv", "mp3", "wav", "flac",
                                "aac", "ogg", "opus", "m4a", "wma",
                            ],
                        )
                        .add_filter(tr.all_files, &["*"])
                        .pick_files()
                    {
                        let added = !paths.is_empty();
                        for p in paths {
                            self.add_file(p);
                        }
                        if added && self.config.autostart_queue && !self.processing {
                            self.start_queue();
                        }
                    }
                }
                ui.label(egui::RichText::new(tr.drop_hint).weak());
            });

            ui.add_space(8.0);

            let highlight = if matches!(self.theme, Theme::Dark) {
                egui::Color32::from_rgb(40, 50, 65)
            } else {
                egui::Color32::from_rgb(205, 222, 245)
            };
            let frame = egui::Frame::group(ui.style()).fill(if hovering_files {
                highlight
            } else {
                ui.style().visuals.extreme_bg_color
            });

            frame.show(ui, |ui| {
                ui.set_min_height(ui.available_height());
                if self.jobs.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(60.0);
                        ui.label(egui::RichText::new(tr.queue_empty).size(18.0).weak());
                        ui.label(egui::RichText::new(tr.queue_empty_hint).weak());
                    });
                    return;
                }

                egui::ScrollArea::vertical().show(ui, |ui| {
                    let mut to_remove: Option<u64> = None;

                    for job in &self.jobs {
                        let name = job.input.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                        let out_name = job.output.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                        let is_pending = matches!(job.status, JobStatus::Pending);

                        // Кнопка удаления прижата вправо; остальное занимает оставшуюся ширину.
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                            if is_pending {
                                if ui.button("🗑").on_hover_text(tr.remove_from_queue).clicked() {
                                    to_remove = Some(job.id);
                                }
                            }

                            ui.vertical(|ui| {
                                ui.set_width(ui.available_width());
                                ui.add(egui::Label::new(egui::RichText::new(&name).strong()).truncate(true));
                                ui.add(
                                    egui::Label::new(egui::RichText::new(format!("→ {out_name}")).weak().small())
                                        .truncate(true),
                                );
                                let info_line = format_media_info(&job.info);
                                if !info_line.is_empty() {
                                    ui.add(
                                        egui::Label::new(egui::RichText::new(info_line).weak().small())
                                            .truncate(true),
                                    );
                                }

                                match &job.status {
                                    JobStatus::Pending => {
                                        ui.label(egui::RichText::new(tr.pending).weak());
                                    }
                                    JobStatus::Running(pct) => {
                                        ui.add(egui::ProgressBar::new(pct / 100.0).show_percentage());
                                    }
                                    JobStatus::Done => {
                                        ui.colored_label(egui::Color32::from_rgb(120, 220, 140), tr.done);
                                    }
                                    JobStatus::Failed(msg) => {
                                        ui.colored_label(egui::Color32::from_rgb(230, 120, 120), format!("✖ {}: {msg}", tr.error));
                                    }
                                }
                            });
                        });
                        ui.separator();
                    }

                    if let Some(id) = to_remove {
                        self.jobs.retain(|j| j.id != id);
                    }
                });
            });
        });

        // Первый запуск (в конфиге ещё нет геометрии): подгоняем высоту окна под настройки,
        // центрируем на мониторе и показываем. На последующих запусках окно уже открыто нужного
        // размера из билдера (main.rs), и обе фазы пропущены — сразу идёт ветка запоминания.
        if !self.startup_size_sent {
            if settings_content_h > 1.0 {
                let header_h = header_resp.response.rect.height();
                let actions_h = actions_resp.response.rect.height();
                // шапка + нижняя панель + заголовок настроек с отступами + контент + небольшой запас
                let needed = (header_h + actions_h + 60.0 + settings_content_h + 24.0).clamp(480.0, 1040.0);
                let width = ctx.screen_rect().width();
                let pos = ctx.input(|i| i.viewport().monitor_size).map(|m| {
                    egui::pos2(((m.x - width) * 0.5).max(0.0), ((m.y - needed) * 0.5).max(0.0))
                });

                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(width, needed)));
                if let Some(p) = pos {
                    ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(p));
                }
                let p = pos.unwrap_or(egui::Pos2::ZERO);
                self.config.geometry = Some([width, needed, p.x, p.y]);
                self.startup_target_h = needed;
                self.startup_size_sent = true;
            }
        } else if !self.startup_shown {
            self.startup_frames += 1;
            // Показываем, когда высота окна совпала с целевой (или через несколько кадров — страховка).
            let reached = (ctx.screen_rect().height() - self.startup_target_h).abs() < 2.0;
            if reached || self.startup_frames > 10 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                self.persist();
                self.startup_shown = true;
            }
        } else {
            // Окно показано: запоминаем изменения размера/позиции, сделанные пользователем.
            let size = ctx.screen_rect().size();
            if let Some(min) = ctx.input(|i| i.viewport().outer_rect.map(|r| r.min)) {
                let cur = [size.x, size.y, min.x, min.y];
                let changed = self.config.geometry.map_or(true, |g| {
                    (g[0] - cur[0]).abs() > 3.0
                        || (g[1] - cur[1]).abs() > 3.0
                        || (g[2] - cur[2]).abs() > 3.0
                        || (g[3] - cur[3]).abs() > 3.0
                });
                if changed {
                    self.config.geometry = Some(cur);
                    self.persist();
                }
            }
        }

        // Окно настроек.
        self.settings_window(ctx, lang);
    }
}
