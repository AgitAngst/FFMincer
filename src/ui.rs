// Отрисовка окна на общем наборе Anvil: шапка, очередь, панель конвертации, кнопки внизу,
// настройки и «О программе».

use anvil_ui::chrome::{self, AboutAction, AppInfo};
use anvil_ui::theme::radius;
use anvil_ui::widgets as w;
use anvil_ui::{Icon, Kind, Lang, Palette, Tone};
use eframe::egui::{self, Margin, RichText, Stroke, Ui};

use crate::app::{self, App, JobStatus};
use crate::config::PostAction;
use crate::texts::{self, BitrateKind, Preset, Tr, tip};

fn info(lang: Lang) -> AppInfo {
    AppInfo {
        name: "FFMincer",
        icon: Icon::Film,
        version: env!("CARGO_PKG_VERSION"),
        tagline: texts::strings(lang).subtitle,
        repository: env!("CARGO_PKG_REPOSITORY"),
    }
}

pub fn draw(app: &mut App, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let lang = app.lang();
    let tr = texts::strings(lang);
    let hovering_files = ctx.input(|i| !i.raw.hovered_files.is_empty());

    let header_h = top_bar(app, ui, lang);
    let actions_h = actions(app, ui, tr);
    let settings_content_h = conversion_panel(app, ui, tr, lang);
    queue(app, ui, tr, hovering_files);

    startup_geometry(app, &ctx, header_h + actions_h, settings_content_h);
    settings_dialog(app, &ctx, lang);
    let info = info(lang);
    let status = anvil_update::ui::about_status(&ctx, &app.updater);
    if chrome::about(&ctx, &mut app.show_about, &info, status.as_deref()) == Some(AboutAction::CheckUpdates) {
        app.updater.check(app.config.common.prerelease, None, true);
    }
}

/// Шапка: знак, название, что делает программа; справа — меню. Возвращает высоту.
fn top_bar(app: &mut App, ui: &mut Ui, lang: Lang) -> f32 {
    let before = ui.available_rect_before_wrap().top();
    chrome::top_bar(ui, |ui| {
        let p = Palette::of(ui);
        chrome::brand(ui, Icon::Film, "FFMincer");
        ui.add_space(10.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let gear = w::icon_button(ui, Icon::Gear, tip(lang, "Меню", "Menu"));
            w::menu(&gear, 240.0, |ui| {
                if w::menu_item(ui, Some(Icon::Gear), tip(lang, "Настройки", "Settings"), Some("Ctrl+,")).clicked()
                {
                    app.show_settings = true;
                }
                if w::menu_item(ui, Some(Icon::Refresh), tip(lang, "Проверить обновления", "Check for updates"), None)
                    .clicked()
                {
                    app.updater.check(app.config.common.prerelease, None, true);
                    app.show_about = true;
                }
                if w::menu_item(ui, Some(Icon::Info), tip(lang, "О программе", "About"), None).clicked() {
                    app.show_about = true;
                }
            });
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let subtitle = RichText::new(texts::strings(lang).subtitle).size(13.0).color(p.weak);
                ui.add(egui::Label::new(subtitle).truncate());
            });
        });
    });
    if ui
        .ctx()
        .input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Comma)))
    {
        app.show_settings = true;
    }
    ui.available_rect_before_wrap().top() - before
}

/// Низ окна: общий прогресс и кнопки очереди. Возвращает высоту.
fn actions(app: &mut App, ui: &mut Ui, tr: &Tr) -> f32 {
    let p = Palette::of(ui);
    egui::Panel::bottom("actions")
        .frame(
            egui::Frame::new()
                .fill(p.surface)
                .inner_margin(Margin::symmetric(16, 10))
                .stroke(Stroke::new(1.0, p.border)),
        )
        .show(ui, |ui| {
            // Общий прогресс по всей очереди.
            let total = app.jobs.len();
            let mut progress_sum = 0.0f32;
            for job in &app.jobs {
                match &job.status {
                    JobStatus::Done | JobStatus::Failed(_) => progress_sum += 1.0,
                    JobStatus::Running(pct) => progress_sum += pct / 100.0,
                    JobStatus::Pending => {}
                }
            }
            if total > 0 && (app.processing || progress_sum > 0.0) {
                let overall = progress_sum / total as f32;
                ui.horizontal(|ui| {
                    ui.label(RichText::new(tr.overall).color(p.weak));
                    let width = (ui.available_width() - 60.0).max(60.0);
                    w::progress(ui, Some(overall), width);
                    ui.label(RichText::new(format!("{:.0}%", overall * 100.0)).color(p.text));
                });
                ui.add_space(4.0);
            }

            ui.horizontal(|ui| {
                let pending_count = app.jobs.iter().filter(|j| matches!(j.status, JobStatus::Pending)).count();
                let can_start = !app.processing && pending_count > 0;
                if ui
                    .add_enabled_ui(can_start, |ui| w::button(ui, Kind::Primary, Some(Icon::Play), tr.start))
                    .inner
                    .clicked()
                {
                    app.start_queue();
                }
                if ui
                    .add_enabled_ui(app.processing, |ui| w::button(ui, Kind::Danger, Some(Icon::Stop), tr.cancel))
                    .inner
                    .clicked()
                {
                    app.cancel_queue();
                }
                let any_done = app.jobs.iter().any(|j| matches!(j.status, JobStatus::Done));
                if ui.add_enabled_ui(any_done, |ui| w::button(ui, Kind::Ghost, None, tr.clear_finished)).inner.clicked()
                {
                    app.clear_finished();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(format!("{}{}", tr.queued, pending_count)).color(p.weak));
                });
            });
        })
        .response
        .rect
        .height()
}

/// Правая панель: пресеты, кодеки, битрейт, контейнер, папка вывода. Возвращает высоту содержимого.
fn conversion_panel(app: &mut App, ui: &mut Ui, tr: &Tr, lang: Lang) -> f32 {
    let p = Palette::of(ui);
    let mut content_h = 0.0;
    egui::Panel::right("settings")
        .resizable(true)
        .default_size(320.0)
        .size_range(260.0..=480.0)
        .frame(
            egui::Frame::new()
                .fill(p.surface)
                .inner_margin(Margin::symmetric(16, 14))
                .stroke(Stroke::new(1.0, p.border)),
        )
        .show(ui, |ui| {
            w::section_label(ui, tr.settings);
            ui.add_space(6.0);
            let out = egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| conversion(app, ui, tr, lang));
            content_h = out.content_size.y;
        });
    content_h
}

fn conversion(app: &mut App, ui: &mut Ui, tr: &Tr, lang: Lang) {
    let p = Palette::of(ui);
    // Ползунки — во всю ширину панели, место оставляем только под поле со значением.
    ui.spacing_mut().slider_width = (ui.available_width() - 90.0).max(80.0);
    let field = |ui: &mut Ui, text: &str| ui.label(RichText::new(text).size(12.5).color(p.weak));
    let hint = |ui: &mut Ui, text: &str| ui.label(RichText::new(text).size(12.0).color(p.weak));

    field(ui, tr.presets).on_hover_text(tip(
        lang,
        "Готовые наборы настроек. Выбор пункта заполняет кодеки, битрейт и контейнер за тебя. Наведи на пункт списка — там описание.",
        "Ready-made setting bundles. Picking one fills in codecs, bitrate and container for you. Hover a list item for its description.",
    ));
    let mut chosen: Option<Preset> = None;
    egui::ComboBox::from_id_salt("preset_combo")
        .width(ui.available_width())
        .selected_text(match app.current_preset {
            Some(p) => p.label(lang),
            None => tr.preset_pick,
        })
        .show_ui(ui, |ui| {
            for p in Preset::ALL {
                if ui
                    .selectable_label(app.current_preset == Some(p), p.label(lang))
                    .on_hover_text(texts::tip_preset(p, lang))
                    .clicked()
                {
                    chosen = Some(p);
                }
            }
        });
    if let Some(p) = chosen {
        app.current_preset = Some(p);
        app.apply_preset(p);
    }

    section(ui, tr.video_codec);
    egui::ComboBox::from_id_salt("video_codec")
        .width(ui.available_width())
        .selected_text(app.settings.video_codec.clone())
        .show_ui(ui, |ui| {
            for c in app::VIDEO_CODECS {
                ui.selectable_value(&mut app.settings.video_codec, c.to_string(), c)
                    .on_hover_text(texts::tip_video_codec(c, lang));
            }
        })
        .response
        .on_hover_text(tip(
            lang,
            "Чем кодируется видео. libx* (CPU) дают качество по CRF; *_nvenc — скорость на GPU по битрейту; copy — без перекодирования; none — убрать видео.",
            "How video is encoded. libx* (CPU) give CRF-based quality; *_nvenc — GPU speed by bitrate; copy — no re-encode; none — drop video.",
        ));

    let video_active = !matches!(app.settings.video_codec.as_str(), "none" | "copy");
    let is_nvenc = app.settings.video_codec.ends_with("_nvenc");

    if video_active {
        w::switch(ui, &mut app.settings.hw_decode, tr.hw_decode).on_hover_text(tip(
            lang,
            "Декодировать вход на GPU (CUDA).\nХорошо: ускоряет NVENC на тяжёлых исходниках.\nПлохо: с редкими кодеками/фильтрами может дать ошибку — тогда выключи.",
            "Decode input on the GPU (CUDA).\nGood: speeds up NVENC on heavy sources.\nBad: can fail with exotic codecs/filters — turn it off then.",
        ));

        let res_tip = tip(
            lang,
            "Масштаб выхода ШИРИНАxВЫСОТА.\nХорошо: 1920x1080, 1280x720.\nПлохо: нечётная высота (1920x1081) ломает H.264; апскейл 720p→4K без смысла. Пусто = как в исходнике.",
            "Output scale WIDTHxHEIGHT.\nGood: 1920x1080, 1280x720.\nBad: odd height (1920x1081) breaks H.264; upscaling 720p→4K is pointless. Empty = same as source.",
        );
        field(ui, tr.resolution).on_hover_text(res_tip);
        ui.add(egui::TextEdit::singleline(&mut app.settings.resolution).desired_width(f32::INFINITY))
            .on_hover_text(res_tip);
        hint(ui, tr.resolution_hint);

        let preset_tip = tip(
            lang,
            "Скорость ↔ сжатие.\nlibx264/5: ultrafast..veryslow (медленнее = меньше файл). NVENC: p1..p7.\nХорошо: medium/slow, p5.\nПлохо: placebo/veryslow ради 1% размера.",
            "Speed ↔ compression.\nlibx264/5: ultrafast..veryslow (slower = smaller file). NVENC: p1..p7.\nGood: medium/slow, p5.\nBad: placebo/veryslow for a 1% gain.",
        );
        field(ui, tr.preset_label).on_hover_text(preset_tip);
        ui.add(egui::TextEdit::singleline(&mut app.settings.preset).desired_width(f32::INFINITY))
            .on_hover_text(preset_tip);
        hint(ui, if is_nvenc { "p1..p7" } else { "ultrafast..veryslow" });

        bitrate_control(ui, tr, lang, tr.video_bitrate, &mut app.settings.video_bitrate, BitrateKind::Video);
        hint(ui, tr.video_bitrate_hint);

        if app.settings.video_bitrate.trim().is_empty() && !is_nvenc {
            field(ui, tr.crf);
            ui.add(egui::Slider::new(&mut app.settings.crf, 0..=51)).on_hover_text(tip(
                lang,
                "Постоянное качество для CPU-кодеков. Меньше = лучше и больше файл.\nХорошо: 18–23 (x264), 20–26 (x265).\nПлохо: 0 (огромный файл), 35+ (мыло).",
                "Constant quality for CPU codecs. Lower = better and bigger file.\nGood: 18–23 (x264), 20–26 (x265).\nBad: 0 (huge file), 35+ (mushy).",
            ));
        }
    }

    section(ui, tr.audio_codec);
    egui::ComboBox::from_id_salt("audio_codec")
        .width(ui.available_width())
        .selected_text(app.settings.audio_codec.clone())
        .show_ui(ui, |ui| {
            for c in app::AUDIO_CODECS {
                ui.selectable_value(&mut app.settings.audio_codec, c.to_string(), c)
                    .on_hover_text(texts::tip_audio_codec(c, lang));
            }
        })
        .response
        .on_hover_text(tip(
            lang,
            "Чем кодируется звук. aac/mp3 — совместимость; opus — лучший звук на низком битрейте; flac — без потерь; copy — без перекодирования; none — убрать звук.",
            "How audio is encoded. aac/mp3 — compatibility; opus — best sound at low bitrate; flac — lossless; copy — no re-encode; none — drop audio.",
        ));

    if !matches!(app.settings.audio_codec.as_str(), "none" | "copy") {
        bitrate_control(ui, tr, lang, tr.audio_bitrate, &mut app.settings.audio_bitrate, BitrateKind::Audio);
        w::switch(ui, &mut app.settings.loudnorm, tr.loudnorm).on_hover_text(tip(
            lang,
            "Приводит громкость к стандарту вещания EBU R128 (-af loudnorm).\nХорошо: разнобой по громкости в подборке роликов/музыки.\nПлохо: уже смастеренный трек — можно испортить динамику. Не работает с кодеком copy.",
            "Brings loudness to the EBU R128 broadcast standard (-af loudnorm).\nGood: a batch of clips/music with uneven volume.\nBad: an already-mastered track — may squash dynamics. Doesn't work with the copy codec.",
        ));
    }

    section(ui, tr.container);
    let mut container_changed = false;
    egui::ComboBox::from_id_salt("container")
        .width(ui.available_width())
        .selected_text(app.container_ext.clone())
        .show_ui(ui, |ui| {
            for c in app::CONTAINERS {
                if ui
                    .selectable_value(&mut app.container_ext, c.to_string(), c)
                    .on_hover_text(texts::tip_container(c, lang))
                    .changed()
                {
                    container_changed = true;
                }
            }
        })
        .response
        .on_hover_text(tip(
            lang,
            "Расширение/формат выходного файла — определяет, какие кодеки в него влезут.\nХорошо: mp4 для раздачи, mkv для архива.\nПлохо: webm с H.264 (кодеки не те).",
            "Output file extension/format — decides which codecs fit inside.\nGood: mp4 for sharing, mkv for archiving.\nBad: webm with H.264 (wrong codecs).",
        ));
    if container_changed {
        app.recompute_pending_outputs();
    }

    if matches!(app.container_ext.as_str(), "mp4" | "mov" | "m4a") {
        w::switch(ui, &mut app.settings.faststart, tr.faststart).on_hover_text(tip(
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
    section(ui, tr.output_folder).on_hover_text(folder_tip);
    let out_dir_text = match &app.output_dir {
        Some(dir) => dir.to_string_lossy().into_owned(),
        None => tr.same_as_source.to_string(),
    };
    ui.add(egui::Label::new(RichText::new(out_dir_text).color(p.weak)).truncate()).on_hover_text(folder_tip);
    ui.horizontal(|ui| {
        if w::button(ui, Kind::Secondary, Some(Icon::Folder), tr.choose_folder).clicked()
            && let Some(dir) = rfd::FileDialog::new().pick_folder()
        {
            app.config.output_dir = dir.display().to_string();
            app.output_dir = Some(dir);
            app.recompute_pending_outputs();
            app.persist();
        }
        if app.output_dir.is_some() && w::button(ui, Kind::Ghost, None, tr.reset).clicked() {
            app.output_dir = None;
            app.config.output_dir.clear();
            app.recompute_pending_outputs();
            app.persist();
        }
    });
}

/// Подзаголовок группы в панели конвертации — с отступом и разделителем.
fn section(ui: &mut Ui, text: &str) -> egui::Response {
    ui.add_space(8.0);
    w::divider(ui);
    ui.add_space(6.0);
    let p = Palette::of(ui);
    ui.label(RichText::new(text.trim_end_matches(':')).font(anvil_ui::semibold(13.5)).color(p.text))
}

/// Единый виджет битрейта: ползунок с частыми значениями + поле для своего значения.
fn bitrate_control(ui: &mut Ui, tr: &Tr, lang: Lang, label: &str, value: &mut String, kind: BitrateKind) {
    let p = Palette::of(ui);
    let options: &[u32] = match kind {
        BitrateKind::Audio => &texts::AUDIO_BITRATES,
        BitrateKind::Video => &texts::VIDEO_BITRATES,
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

    ui.label(RichText::new(label).size(12.5).color(p.weak));

    let mut idx = texts::parse_kbps(value).map(|c| texts::nearest_index(options, c)).unwrap_or(options.len() / 2);
    let last = options.len() - 1;
    let opts: Vec<u32> = options.to_vec();

    let resp = ui
        .add(
            egui::Slider::new(&mut idx, 0..=last)
                .custom_formatter(move |n, _| {
                    let i = (n.round() as usize).min(opts.len() - 1);
                    texts::format_bitrate(opts[i], kind)
                })
                .custom_parser(|_| None),
        )
        .on_hover_text(slider_tip);
    if resp.changed() {
        *value = format!("{}k", options[idx]);
    }

    ui.horizontal(|ui| {
        ui.label(RichText::new(tr.custom_bitrate).size(12.0).color(p.weak));
        ui.add(egui::TextEdit::singleline(value).desired_width(f32::INFINITY)).on_hover_text(custom_tip);
    });
}

/// Середина: баннер обновления, «Добавить файлы…» и очередь.
fn queue(app: &mut App, ui: &mut Ui, tr: &Tr, hovering_files: bool) {
    let p = Palette::of(ui);
    egui::CentralPanel::no_frame().frame(egui::Frame::new().fill(p.bg).inner_margin(Margin::symmetric(20, 16))).show(
        ui,
        |ui| {
            let updater = app.updater.clone();
            if anvil_update::ui::banner(ui, &updater, &mut app.config.common) {
                app.persist();
            }
            ui.horizontal(|ui| {
                if w::button(ui, Kind::Secondary, Some(Icon::Plus), tr.add_files).clicked()
                    && let Some(paths) = rfd::FileDialog::new()
                        .add_filter(tr.media_filter, &app::MEDIA_EXTENSIONS)
                        .add_filter(tr.all_files, &["*"])
                        .pick_files()
                {
                    app.add_files(paths);
                }
                ui.label(RichText::new(tr.drop_hint).color(p.weak));
            });
            ui.add_space(10.0);

            let (fill, stroke) = if hovering_files {
                (p.soft(p.accent), Stroke::new(1.5, p.accent))
            } else {
                (p.card, Stroke::new(1.0, p.border))
            };
            egui::Frame::new()
                .fill(fill)
                .stroke(stroke)
                .corner_radius(radius::CARD)
                .inner_margin(Margin::same(14))
                .show(ui, |ui| {
                    ui.set_min_height(ui.available_height());
                    ui.set_width(ui.available_width());
                    if app.jobs.is_empty() {
                        ui.add_space(40.0);
                        w::empty_state(ui, Icon::Film, tr.queue_empty, tr.queue_empty_hint);
                        return;
                    }
                    jobs(app, ui, tr);
                });
        },
    );
}

fn jobs(app: &mut App, ui: &mut Ui, tr: &Tr) {
    let p = Palette::of(ui);
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let mut to_remove: Option<u64> = None;
        let count = app.jobs.len();
        for (i, job) in app.jobs.iter().enumerate() {
            let name = job.input.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let out_name = job.output.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let is_pending = matches!(job.status, JobStatus::Pending);

            // Кнопка удаления прижата вправо; остальное занимает оставшуюся ширину.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                if is_pending && w::icon_button(ui, Icon::Trash, tr.remove_from_queue).clicked() {
                    to_remove = Some(job.id);
                }
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width());
                    ui.add(
                        egui::Label::new(RichText::new(&name).font(anvil_ui::semibold(14.0)).color(p.text)).truncate(),
                    )
                    .on_hover_text(job.input.display().to_string());
                    ui.add(
                        egui::Label::new(RichText::new(format!("→ {out_name}")).size(12.5).color(p.weak)).truncate(),
                    )
                    .on_hover_text(job.output.display().to_string());
                    let info_line = texts::format_media_info(&job.info);
                    if !info_line.is_empty() {
                        ui.add(egui::Label::new(RichText::new(info_line).size(12.5).color(p.weak)).truncate());
                    }
                    ui.add_space(2.0);
                    match &job.status {
                        JobStatus::Pending => {
                            w::badge(ui, tr.pending, Tone::Neutral);
                        }
                        JobStatus::Running(pct) => {
                            ui.horizontal(|ui| {
                                let width = (ui.available_width() - 50.0).max(40.0);
                                w::progress(ui, Some(pct / 100.0), width);
                                ui.label(RichText::new(format!("{pct:.0}%")).color(p.text));
                            });
                        }
                        JobStatus::Done => {
                            w::badge(ui, tr.done, Tone::Success);
                        }
                        JobStatus::Failed(msg) => {
                            ui.horizontal(|ui| {
                                w::badge(ui, tr.error, Tone::Danger);
                                ui.add(egui::Label::new(RichText::new(msg).color(p.danger)).truncate())
                                    .on_hover_text(msg);
                            });
                        }
                    }
                });
            });
            if i + 1 < count {
                ui.add_space(6.0);
                w::divider(ui);
                ui.add_space(6.0);
            }
        }

        if let Some(id) = to_remove {
            app.jobs.retain(|j| j.id != id);
        }
    });
}

/// Окно настроек: общие для семьи (тема, язык, обновления) и свои — поведение, ffmpeg, вывод.
fn settings_dialog(app: &mut App, ctx: &egui::Context, lang: Lang) {
    if !app.show_settings {
        app.autostart_cached = None;
        return;
    }
    let mut open = app.show_settings;
    chrome::dialog(ctx, "settings", tip(lang, "Настройки", "Settings"), 540.0, &mut open, |ui| {
        if chrome::common_settings(ui, &mut app.config.common) {
            app.persist();
        }
        ui.add_space(10.0);
        let lang = app.lang();

        // ===== Приложение =====
        w::section_label(ui, tip(lang, "Приложение", "Application"));
        ui.add_space(2.0);
        chrome::setting_row(ui, tip(lang, "По завершении", "When finished"), |ui| {
            let mut act = app.config.post_action;
            egui::ComboBox::from_id_salt("cfg_post").width(220.0).selected_text(post_action_label(act, lang)).show_ui(
                ui,
                |ui| {
                    for a in [PostAction::None, PostAction::OpenFolder, PostAction::Sleep, PostAction::Shutdown] {
                        ui.selectable_value(&mut act, a, post_action_label(a, lang));
                    }
                },
            );
            if act != app.config.post_action {
                app.config.post_action = act;
                app.persist();
            }
        });
        let mut autostart = *app.autostart_cached.get_or_insert_with(crate::sys::is_autostart_enabled);
        if w::switch(ui, &mut autostart, tip(lang, "Запускать вместе с системой", "Launch at system startup")).changed()
        {
            let ok = crate::sys::set_autostart(autostart);
            app.autostart_cached = Some(if ok { autostart } else { !autostart });
        }
        let autostart_queue =
            tip(lang, "Начинать конвертацию сразу при добавлении", "Start converting as soon as files are added");
        if w::switch(ui, &mut app.config.autostart_queue, autostart_queue).changed() {
            app.persist();
        }
        let autoclear = tip(lang, "Убирать завершённые из списка", "Remove finished from the list");
        if w::switch(ui, &mut app.config.autoclear_finished, autoclear).changed() {
            app.persist();
        }
        let sound = tip(lang, "Звук по завершении очереди", "Sound when the queue finishes");
        if w::switch(ui, &mut app.config.sound_on_finish, sound).changed() {
            app.persist();
        }

        // ===== ffmpeg =====
        ui.add_space(10.0);
        w::section_label(ui, "ffmpeg");
        ui.add_space(4.0);
        let p = Palette::of(ui);
        ui.horizontal(|ui| {
            if w::button(ui, Kind::Secondary, Some(Icon::Refresh), tip(lang, "Проверить", "Check")).clicked() {
                app.check_ffmpeg();
            }
            match &app.ffmpeg_version {
                Some(v) => {
                    ui.add(egui::Label::new(RichText::new(v).color(p.success)).truncate());
                }
                None => {
                    let text = if app.ffmpeg_checked {
                        tip(lang, "не найден", "not found")
                    } else {
                        tip(lang, "не проверялось", "not checked")
                    };
                    ui.label(RichText::new(text).color(if app.ffmpeg_checked { p.danger } else { p.weak }));
                }
            }
        });
        ui.horizontal(|ui| {
            let install = tip(lang, "Установить тихо (winget)", "Install silently (winget)");
            if ui
                .add_enabled_ui(!app.installing, |ui| w::button(ui, Kind::Secondary, Some(Icon::Download), install))
                .inner
                .clicked()
            {
                app.start_ffmpeg_install();
            }
            if app.installing {
                w::spinner(ui, 16.0);
                ui.label(RichText::new(tip(lang, "установка…", "installing…")).color(p.weak));
            } else if !app.install_msg.is_empty() {
                ui.label(RichText::new(&app.install_msg).color(p.weak));
            }
        });
        ui.add_space(4.0);
        path_row(app, ui, tip(lang, "Путь ffmpeg", "ffmpeg path"), true);
        path_row(app, ui, tip(lang, "Путь ffprobe", "ffprobe path"), false);
        chrome::setting_row(ui, tip(lang, "Потоки (-threads)", "Threads (-threads)"), |ui| {
            ui.label(RichText::new(tip(lang, "0 = авто", "0 = auto")).size(12.5).color(p.weak));
            let mut threads = app.config.threads as i32;
            if ui.add(egui::DragValue::new(&mut threads).range(0..=64)).changed() {
                app.config.threads = threads.max(0) as u32;
                app.settings.threads = app.config.threads;
                app.persist();
            }
        });
        let low = tip(lang, "Низкий приоритет процесса ffmpeg", "Low ffmpeg process priority");
        if w::switch(ui, &mut app.config.low_priority, low).changed() {
            app.persist();
        }

        // ===== Вывод =====
        ui.add_space(10.0);
        w::section_label(ui, tip(lang, "Вывод", "Output"));
        ui.add_space(2.0);
        chrome::setting_row(ui, tip(lang, "Шаблон имени", "Name template"), |ui| {
            let edit = egui::TextEdit::singleline(&mut app.config.name_template)
                .desired_width(220.0)
                .hint_text("{name}_converted");
            if ui.add(edit).lost_focus() {
                if app.config.name_template.trim().is_empty() {
                    app.config.name_template = "{name}".into();
                }
                app.persist();
                app.recompute_pending_outputs();
            }
        });
        w::note(ui, tip(lang, "{name} — имя исходного файла", "{name} — source file name"));
        chrome::setting_row(ui, tip(lang, "При совпадении", "On conflict"), |ui| {
            let mut overwrite = app.config.overwrite;
            let options = [
                (true, None, tip(lang, "перезаписывать", "overwrite")),
                (false, None, tip(lang, "переименовывать", "rename")),
            ];
            if w::segmented(ui, &mut overwrite, &options).changed() {
                app.config.overwrite = overwrite;
                app.persist();
                app.recompute_pending_outputs();
            }
        });
        let metadata = tip(lang, "Сохранять метаданные исходника", "Keep source metadata");
        if w::switch(ui, &mut app.config.keep_metadata, metadata).changed() {
            app.settings.keep_metadata = app.config.keep_metadata;
            app.persist();
        }
    });
    app.show_settings = open;
    if !open {
        app.autostart_cached = None;
    }
}

/// Путь к ffmpeg или ffprobe: поле и выбор файла.
fn path_row(app: &mut App, ui: &mut Ui, label: &str, ffmpeg: bool) {
    let lang = app.lang();
    chrome::setting_row(ui, label, |ui| {
        let hint = if ffmpeg { "ffmpeg" } else { "ffprobe" };
        if w::icon_button(ui, Icon::Folder, tip(lang, "Выбрать файл…", "Choose file…")).clicked()
            && let Some(path) = rfd::FileDialog::new().add_filter("exe", &["exe"]).pick_file()
        {
            let value = path.display().to_string();
            if ffmpeg {
                app.config.ffmpeg_path = value;
            } else {
                app.config.ffprobe_path = value;
            }
            app.persist();
        }
        let value = if ffmpeg { &mut app.config.ffmpeg_path } else { &mut app.config.ffprobe_path };
        if ui.add(egui::TextEdit::singleline(value).desired_width(220.0).hint_text(hint)).lost_focus() {
            app.persist();
        }
    });
}

fn post_action_label(action: PostAction, lang: Lang) -> &'static str {
    match action {
        PostAction::None => tip(lang, "Ничего", "Nothing"),
        PostAction::OpenFolder => tip(lang, "Открыть папку вывода", "Open output folder"),
        PostAction::Sleep => tip(lang, "Сон ПК", "Sleep PC"),
        PostAction::Shutdown => tip(lang, "Выключить ПК", "Shut down PC"),
    }
}

/// Первый запуск (в конфиге ещё нет геометрии): подгоняем высоту окна под панель конвертации,
/// центрируем на мониторе и показываем. Дальше — запоминаем размер и место, выбранные пользователем.
fn startup_geometry(app: &mut App, ctx: &egui::Context, bars_h: f32, settings_content_h: f32) {
    if !app.startup_size_sent {
        if settings_content_h > 1.0 {
            // шапка + нижняя панель + заголовок панели с отступами + содержимое + небольшой запас
            let needed = (bars_h + 60.0 + settings_content_h + 24.0).clamp(480.0, 1040.0);
            let width = ctx.content_rect().width();
            let pos = ctx
                .input(|i| i.viewport().monitor_size)
                .map(|m| egui::pos2(((m.x - width) * 0.5).max(0.0), ((m.y - needed) * 0.5).max(0.0)));

            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(width, needed)));
            if let Some(p) = pos {
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(p));
            }
            let p = pos.unwrap_or(egui::Pos2::ZERO);
            app.config.geometry = Some([width, needed, p.x, p.y]);
            app.startup_target_h = needed;
            app.startup_size_sent = true;
        }
    } else if !app.startup_shown {
        app.startup_frames += 1;
        // Показываем, когда высота окна совпала с целевой (или через несколько кадров — страховка).
        let reached = (ctx.content_rect().height() - app.startup_target_h).abs() < 2.0;
        if reached || app.startup_frames > 10 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            app.persist();
            app.startup_shown = true;
        }
    } else {
        // Окно показано: запоминаем изменения размера/позиции, сделанные пользователем.
        let size = ctx.content_rect().size();
        if let Some(min) = ctx.input(|i| i.viewport().outer_rect.map(|r| r.min)) {
            let cur = [size.x, size.y, min.x, min.y];
            let changed = app.config.geometry.is_none_or(|g| g.iter().zip(cur).any(|(a, b)| (a - b).abs() > 3.0));
            if changed {
                app.config.geometry = Some(cur);
                app.persist();
            }
        }
    }
}
