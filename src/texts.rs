// Тексты интерфейса на двух языках и форматирование чисел для показа.
//
// Строки идут парой прямо в коде: `tip(lang, "по-русски", "in English")`. Язык — общий с набором
// Anvil (`anvil_ui::Lang`), выбирается в «Настройках».

use anvil_ui::Lang;

use crate::ffmpeg::MediaInfo;

#[derive(Clone, Copy, PartialEq)]
pub enum Preset {
    Mp4Cpu,
    Mp4Nvenc,
    Mkv265,
    Mp3,
    Flac,
}

impl Preset {
    pub const ALL: [Preset; 5] = [Preset::Mp4Cpu, Preset::Mp4Nvenc, Preset::Mkv265, Preset::Mp3, Preset::Flac];

    pub fn label(self, lang: Lang) -> &'static str {
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
pub struct Tr {
    pub subtitle: &'static str,
    pub settings: &'static str,
    pub presets: &'static str,
    pub preset_pick: &'static str,
    pub video_codec: &'static str,
    pub hw_decode: &'static str,
    pub resolution: &'static str,
    pub resolution_hint: &'static str,
    pub preset_label: &'static str,
    pub video_bitrate: &'static str,
    pub video_bitrate_hint: &'static str,
    pub crf: &'static str,
    pub audio_codec: &'static str,
    pub audio_bitrate: &'static str,
    pub container: &'static str,
    pub output_folder: &'static str,
    pub same_as_source: &'static str,
    pub choose_folder: &'static str,
    pub reset: &'static str,
    pub start: &'static str,
    pub cancel: &'static str,
    pub clear_finished: &'static str,
    pub queued: &'static str,
    pub add_files: &'static str,
    pub drop_hint: &'static str,
    pub media_filter: &'static str,
    pub all_files: &'static str,
    pub queue_empty: &'static str,
    pub queue_empty_hint: &'static str,
    pub pending: &'static str,
    pub done: &'static str,
    pub error: &'static str,
    pub remove_from_queue: &'static str,
    pub overall: &'static str,
    pub custom_bitrate: &'static str,
    pub loudnorm: &'static str,
    pub faststart: &'static str,
}

const RU: Tr = Tr {
    subtitle: "конвертация аудио и видео через ffmpeg",
    settings: "Конвертация",
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
    start: "Начать конвертацию",
    cancel: "Отменить",
    clear_finished: "Очистить завершённые",
    queued: "В очереди: ",
    add_files: "Добавить файлы…",
    drop_hint: "или перетащите файлы сюда",
    media_filter: "Медиа",
    all_files: "Все файлы",
    queue_empty: "Очередь пуста",
    queue_empty_hint: "Перетащите видео или аудио файлы в это окно",
    pending: "Ожидание",
    done: "Готово",
    error: "Ошибка",
    remove_from_queue: "Убрать из очереди",
    overall: "Общий прогресс:",
    custom_bitrate: "своё:",
    loudnorm: "Нормализовать громкость",
    faststart: "Быстрый старт (faststart)",
};

const EN: Tr = Tr {
    subtitle: "audio & video conversion via ffmpeg",
    settings: "Conversion",
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
    start: "Start conversion",
    cancel: "Cancel",
    clear_finished: "Clear finished",
    queued: "Queued: ",
    add_files: "Add files…",
    drop_hint: "or drop files here",
    media_filter: "Media",
    all_files: "All files",
    queue_empty: "Queue is empty",
    queue_empty_hint: "Drop video or audio files onto this window",
    pending: "Pending",
    done: "Done",
    error: "Error",
    remove_from_queue: "Remove from queue",
    overall: "Overall progress:",
    custom_bitrate: "custom:",
    loudnorm: "Normalize loudness",
    faststart: "Fast start (faststart)",
};

pub fn strings(lang: Lang) -> &'static Tr {
    match lang {
        Lang::Ru => &RU,
        Lang::En => &EN,
    }
}

#[derive(Clone, Copy)]
pub enum BitrateKind {
    Audio,
    Video,
}

/// Частые значения битрейта в кбит/с.
pub const AUDIO_BITRATES: [u32; 6] = [96, 128, 160, 192, 256, 320];
pub const VIDEO_BITRATES: [u32; 9] = [1000, 2000, 4000, 6000, 8000, 12000, 16000, 25000, 50000];

/// Разбирает строку битрейта ("192k", "8M", "8000") в кбит/с.
pub fn parse_kbps(s: &str) -> Option<u32> {
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

pub fn nearest_index(options: &[u32], target: u32) -> usize {
    options.iter().enumerate().min_by_key(|(_, v)| (**v as i64 - target as i64).abs()).map(|(i, _)| i).unwrap_or(0)
}

pub fn fmt_duration(secs: f64) -> String {
    let total = secs.max(0.0) as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

pub fn fmt_size(bytes: u64) -> String {
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
pub fn format_media_info(info: &MediaInfo) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let (Some(w), Some(h)) = (info.width, info.height) {
        parts.push(format!("{w}×{h}"));
    }
    let codecs: Vec<&str> = [info.v_codec.as_deref(), info.a_codec.as_deref()].into_iter().flatten().collect();
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

pub fn format_bitrate(kbps: u32, kind: BitrateKind) -> String {
    match kind {
        BitrateKind::Audio => format!("{kbps} kbps"),
        BitrateKind::Video => {
            if kbps.is_multiple_of(1000) {
                format!("{} Mbps", kbps / 1000)
            } else {
                format!("{:.1} Mbps", kbps as f64 / 1000.0)
            }
        }
    }
}

/// Выбирает подсказку по текущему языку.
pub fn tip(lang: Lang, ru: &'static str, en: &'static str) -> &'static str {
    match lang {
        Lang::Ru => ru,
        Lang::En => en,
    }
}

pub fn tip_video_codec(code: &str, lang: Lang) -> &'static str {
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

pub fn tip_audio_codec(code: &str, lang: Lang) -> &'static str {
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

pub fn tip_container(ext: &str, lang: Lang) -> &'static str {
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

pub fn tip_preset(preset: Preset, lang: Lang) -> &'static str {
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
