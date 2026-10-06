// Взаимодействие с ОС: автозапуск, проверка/установка ffmpeg,
// звук и действия по завершении очереди. Всё через внешние утилиты, без доп. зависимостей.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Скрыть консольное окно дочернего процесса (иначе мигает при скрытой консоли).
#[cfg(windows)]
fn hidden(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hidden(_cmd: &mut Command) {}

pub const AUTOSTART_NAME: &str = "FFMincer";
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

/// Есть ли исполняемый файл `name` в папках PATH текущего процесса.
fn on_path(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| dir.join(format!("{name}.exe")).is_file() || dir.join(name).is_file())
    })
}

/// Что запускать для `name` (ffmpeg / ffprobe): путь из настроек; иначе из PATH; иначе из папки
/// ссылок WinGet. После тихой установки через winget PATH уже запущенного процесса не обновляется —
/// без этой папки приложение не увидело бы только что поставленный ffmpeg до перезапуска.
pub fn resolve_tool(configured: &str, name: &str) -> String {
    let configured = configured.trim();
    if !configured.is_empty() {
        return configured.to_string();
    }
    if !on_path(name)
        && let Some(local) = std::env::var_os("LOCALAPPDATA")
    {
        let link: PathBuf = [local.as_os_str(), "Microsoft".as_ref(), "WinGet".as_ref(), "Links".as_ref()]
            .iter()
            .collect::<PathBuf>()
            .join(format!("{name}.exe"));
        if link.is_file() {
            return link.display().to_string();
        }
    }
    name.to_string()
}

/// Первая строка вывода `ffmpeg -version`, если ffmpeg доступен.
pub fn ffmpeg_version(ffmpeg: &str) -> Option<String> {
    let mut cmd = Command::new(resolve_tool(ffmpeg, "ffmpeg"));
    cmd.arg("-version");
    hidden(&mut cmd);
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).lines().next().map(|l| l.trim().to_string())
}

/// Стоит ли автозапуск в реестре (ключ Run).
pub fn is_autostart_enabled() -> bool {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("reg");
        cmd.args(["query", RUN_KEY, "/v", AUTOSTART_NAME]);
        hidden(&mut cmd);
        return cmd.output().map(|o| o.status.success()).unwrap_or(false);
    }
    #[allow(unreachable_code)]
    false
}

/// Включить/выключить автозапуск вместе с системой (ключ Run текущего пользователя).
pub fn set_autostart(enabled: bool) -> bool {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("reg");
        if enabled {
            let Ok(exe) = std::env::current_exe() else {
                return false;
            };
            let val = format!("\"{}\"", exe.display());
            cmd.args(["add", RUN_KEY, "/v", AUTOSTART_NAME, "/t", "REG_SZ", "/d", &val, "/f"]);
        } else {
            cmd.args(["delete", RUN_KEY, "/v", AUTOSTART_NAME, "/f"]);
        }
        hidden(&mut cmd);
        return cmd.status().map(|s| s.success()).unwrap_or(false);
    }
    #[allow(unreachable_code)]
    false
}

/// Тихая установка ffmpeg через winget (блокирующая — вызывать в отдельном потоке).
/// Ошибка — короткая строка для показа как есть: код winget или системная ошибка запуска.
pub fn install_ffmpeg() -> Result<(), String> {
    let mut cmd = Command::new("winget");
    cmd.args([
        "install",
        "--silent",
        "--accept-package-agreements",
        "--accept-source-agreements",
        "-e",
        "--id",
        "Gyan.FFmpeg",
    ]);
    hidden(&mut cmd);
    match cmd.output() {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(format!("winget: {}", o.status.code().unwrap_or(-1))),
        Err(e) => Err(format!("winget: {e}")),
    }
}

pub fn open_folder(path: &Path) {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("explorer");
        cmd.arg(path);
        hidden(&mut cmd);
        let _ = cmd.spawn();
    }
    #[cfg(not(windows))]
    let _ = path;
}

pub fn play_finish_sound() {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("powershell");
        cmd.args(["-NoProfile", "-Command", "[System.Media.SystemSounds]::Asterisk.Play()"]);
        hidden(&mut cmd);
        let _ = cmd.spawn();
    }
}

/// Сон ПК.
pub fn sleep_pc() {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("rundll32.exe");
        cmd.args(["powrprof.dll,SetSuspendState", "0,1,0"]);
        hidden(&mut cmd);
        let _ = cmd.spawn();
    }
}

/// Выключение ПК с задержкой 30 c (отменяется `shutdown /a`).
pub fn shutdown_pc() {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("shutdown");
        cmd.args(["/s", "/t", "30"]);
        hidden(&mut cmd);
        let _ = cmd.spawn();
    }
}
