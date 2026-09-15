use crate::ffmpeg::{self, Event, JobSettings, QueueItem};
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
}

pub struct App {
    jobs: Vec<Job>,
    next_id: u64,

    settings: JobSettings,
    container_ext: String,
    output_dir: Option<PathBuf>,

    processing: bool,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    stop_flag: Arc<AtomicBool>,
    current_child: Arc<Mutex<Option<std::process::Child>>>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        apply_theme(&cc.egui_ctx);
        let (tx, rx) = std::sync::mpsc::channel();
        Self {
            jobs: Vec::new(),
            next_id: 1,
            settings: JobSettings::default(),
            container_ext: "mp4".into(),
            output_dir: None,
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
        let output = ffmpeg::compute_output_path(&path, &self.container_ext, self.output_dir.as_deref());
        self.jobs.push(Job {
            id: self.next_id,
            input: path,
            output,
            status: JobStatus::Pending,
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
        for job in self.jobs.iter_mut() {
            if matches!(job.status, JobStatus::Pending) {
                job.output = ffmpeg::compute_output_path(&job.input, &self.container_ext, self.output_dir.as_deref());
            }
        }
    }

    fn drain_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::Started(id) => self.set_status(id, JobStatus::Running(0.0)),
                Event::Progress(id, pct) => self.set_status(id, JobStatus::Running(pct)),
                Event::Done(id) => self.set_status(id, JobStatus::Done),
                Event::Failed(id, msg) => self.set_status(id, JobStatus::Failed(msg)),
                Event::QueueFinished => self.processing = false,
            }
        }
    }

    fn set_status(&mut self, id: u64, status: JobStatus) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
            job.status = status;
        }
    }

    fn start_queue(&mut self) {
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
        std::thread::spawn(move || {
            ffmpeg::run_queue(items, tx, stop_flag, current_child);
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
}

enum Preset {
    Mp4Cpu,
    Mp4Nvenc,
    Mkv265,
    Mp3,
    Flac,
}

fn apply_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.window_rounding = egui::Rounding::same(12.0);
    visuals.widgets.noninteractive.rounding = egui::Rounding::same(8.0);
    visuals.widgets.inactive.rounding = egui::Rounding::same(8.0);
    visuals.widgets.hovered.rounding = egui::Rounding::same(8.0);
    visuals.widgets.active.rounding = egui::Rounding::same(8.0);
    visuals.selection.bg_fill = egui::Color32::from_rgb(88, 166, 255);
    visuals.hyperlink_color = egui::Color32::from_rgb(88, 166, 255);
    visuals.panel_fill = egui::Color32::from_rgb(24, 26, 32);
    visuals.window_fill = egui::Color32::from_rgb(24, 26, 32);
    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.spacing.button_padding = egui::vec2(14.0, 7.0);
    ctx.set_style(style);
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();
        if self.processing {
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }

        // Drag-and-drop файлов из проводника.
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        for path in dropped {
            self.add_file(path);
        }
        let hovering_files = ctx.input(|i| !i.raw.hovered_files.is_empty());

        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading("🥩 FFMincer");
                ui.label(egui::RichText::new("конвертация аудио и видео через ffmpeg").weak());
            });
            ui.add_space(6.0);
        });

        egui::SidePanel::right("settings").min_width(280.0).show(ctx, |ui| {
            ui.add_space(8.0);
            ui.heading("Настройки");
            ui.add_space(4.0);

            ui.label("Быстрые пресеты:");
            ui.horizontal_wrapped(|ui| {
                if ui.button("MP4 (CPU, H.264)").clicked() {
                    self.apply_preset(Preset::Mp4Cpu);
                }
                if ui.button("MP4 (NVENC, быстро)").clicked() {
                    self.apply_preset(Preset::Mp4Nvenc);
                }
                if ui.button("MKV (H.265)").clicked() {
                    self.apply_preset(Preset::Mkv265);
                }
                if ui.button("MP3 (аудио)").clicked() {
                    self.apply_preset(Preset::Mp3);
                }
                if ui.button("FLAC (без потерь)").clicked() {
                    self.apply_preset(Preset::Flac);
                }
            });

            ui.separator();

            egui::ComboBox::from_label("Видеокодек")
                .selected_text(self.settings.video_codec.clone())
                .show_ui(ui, |ui| {
                    for c in VIDEO_CODECS {
                        ui.selectable_value(&mut self.settings.video_codec, c.to_string(), c);
                    }
                });

            let video_active = !matches!(self.settings.video_codec.as_str(), "none" | "copy");
            let is_nvenc = self.settings.video_codec.ends_with("_nvenc");

            if video_active {
                ui.checkbox(&mut self.settings.hw_decode, "Аппаратное декодирование (CUDA)");

                ui.horizontal(|ui| {
                    ui.label("Разрешение:");
                    ui.text_edit_singleline(&mut self.settings.resolution);
                    ui.label(egui::RichText::new("(например 1920x1080, пусто = как есть)").weak());
                });

                ui.horizontal(|ui| {
                    ui.label("Preset:");
                    ui.text_edit_singleline(&mut self.settings.preset);
                    ui.label(egui::RichText::new(if is_nvenc { "p1..p7" } else { "ultrafast..veryslow" }).weak());
                });

                ui.horizontal(|ui| {
                    ui.label("Видеобитрейт:");
                    ui.text_edit_singleline(&mut self.settings.video_bitrate);
                    ui.label(egui::RichText::new("напр. 8M (пусто = CRF)").weak());
                });

                if self.settings.video_bitrate.trim().is_empty() && !is_nvenc {
                    ui.add(egui::Slider::new(&mut self.settings.crf, 0..=51).text("CRF (меньше = лучше)"));
                }
            }

            ui.separator();

            egui::ComboBox::from_label("Аудиокодек")
                .selected_text(self.settings.audio_codec.clone())
                .show_ui(ui, |ui| {
                    for c in AUDIO_CODECS {
                        ui.selectable_value(&mut self.settings.audio_codec, c.to_string(), c);
                    }
                });

            if !matches!(self.settings.audio_codec.as_str(), "none" | "copy") {
                ui.horizontal(|ui| {
                    ui.label("Аудиобитрейт:");
                    ui.text_edit_singleline(&mut self.settings.audio_bitrate);
                });
            }

            ui.separator();

            egui::ComboBox::from_label("Контейнер (расширение)")
                .selected_text(self.container_ext.clone())
                .show_ui(ui, |ui| {
                    for c in CONTAINERS {
                        if ui.selectable_value(&mut self.container_ext, c.to_string(), c).changed() {
                            for job in self.jobs.iter_mut() {
                                if matches!(job.status, JobStatus::Pending) {
                                    job.output = ffmpeg::compute_output_path(
                                        &job.input,
                                        &self.container_ext,
                                        self.output_dir.as_deref(),
                                    );
                                }
                            }
                        }
                    }
                });

            ui.horizontal(|ui| {
                ui.label("Папка вывода:");
                match &self.output_dir {
                    Some(dir) => {
                        ui.label(egui::RichText::new(dir.to_string_lossy()).weak());
                    }
                    None => {
                        ui.label(egui::RichText::new("как у исходника").weak());
                    }
                }
            });
            ui.horizontal(|ui| {
                if ui.button("Выбрать папку…").clicked() {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        self.output_dir = Some(dir);
                        for job in self.jobs.iter_mut() {
                            if matches!(job.status, JobStatus::Pending) {
                                job.output = ffmpeg::compute_output_path(
                                    &job.input,
                                    &self.container_ext,
                                    self.output_dir.as_deref(),
                                );
                            }
                        }
                    }
                }
                if self.output_dir.is_some() && ui.button("Сбросить").clicked() {
                    self.output_dir = None;
                    for job in self.jobs.iter_mut() {
                        if matches!(job.status, JobStatus::Pending) {
                            job.output = ffmpeg::compute_output_path(&job.input, &self.container_ext, None);
                        }
                    }
                }
            });
        });

        egui::TopBottomPanel::bottom("actions").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let pending_count = self.jobs.iter().filter(|j| matches!(j.status, JobStatus::Pending)).count();

                if ui
                    .add_enabled(!self.processing && pending_count > 0, egui::Button::new("▶ Начать конвертацию"))
                    .clicked()
                {
                    self.start_queue();
                }

                if ui.add_enabled(self.processing, egui::Button::new("⏹ Отменить")).clicked() {
                    self.cancel_queue();
                }

                if ui.button("Очистить завершённые").clicked() {
                    self.clear_finished();
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("В очереди: {}", pending_count));
                });
            });
            ui.add_space(6.0);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("➕ Добавить файлы…").clicked() {
                    if let Some(paths) = rfd::FileDialog::new()
                        .add_filter(
                            "Медиа",
                            &[
                                "mp4", "mkv", "mov", "avi", "webm", "flv", "m4v", "wmv", "mp3", "wav", "flac",
                                "aac", "ogg", "opus", "m4a", "wma",
                            ],
                        )
                        .add_filter("Все файлы", &["*"])
                        .pick_files()
                    {
                        for p in paths {
                            self.add_file(p);
                        }
                    }
                }
                ui.label(egui::RichText::new("или перетащите файлы сюда").weak());
            });

            ui.add_space(8.0);

            let frame = egui::Frame::group(ui.style()).fill(if hovering_files {
                egui::Color32::from_rgb(40, 50, 65)
            } else {
                ui.style().visuals.extreme_bg_color
            });

            frame.show(ui, |ui| {
                ui.set_min_height(ui.available_height());
                if self.jobs.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(60.0);
                        ui.label(egui::RichText::new("📂 Очередь пуста").size(18.0).weak());
                        ui.label(egui::RichText::new("Перетащите видео или аудио файлы в это окно").weak());
                    });
                    return;
                }

                egui::ScrollArea::vertical().show(ui, |ui| {
                    let mut to_remove: Option<u64> = None;

                    for job in &self.jobs {
                        ui.horizontal(|ui| {
                            let name = job.input.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                            let out_name = job.output.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();

                            ui.set_min_width(ui.available_width() - 90.0);
                            ui.vertical(|ui| {
                                ui.label(egui::RichText::new(&name).strong());
                                ui.label(egui::RichText::new(format!("→ {out_name}")).weak().small());

                                match &job.status {
                                    JobStatus::Pending => {
                                        ui.label(egui::RichText::new("Ожидание").weak());
                                    }
                                    JobStatus::Running(pct) => {
                                        ui.add(egui::ProgressBar::new(pct / 100.0).show_percentage());
                                    }
                                    JobStatus::Done => {
                                        ui.colored_label(egui::Color32::from_rgb(120, 220, 140), "✔ Готово");
                                    }
                                    JobStatus::Failed(msg) => {
                                        ui.colored_label(egui::Color32::from_rgb(230, 120, 120), format!("✖ Ошибка: {msg}"));
                                    }
                                }
                            });

                            if matches!(job.status, JobStatus::Pending) {
                                if ui.button("✕").on_hover_text("Убрать из очереди").clicked() {
                                    to_remove = Some(job.id);
                                }
                            }
                        });
                        ui.separator();
                    }

                    if let Some(id) = to_remove {
                        self.jobs.retain(|j| j.id != id);
                    }
                });
            });
        });
    }
}
