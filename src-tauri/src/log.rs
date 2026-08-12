use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

const MAX_BYTES: u64 = 4 * 1024 * 1024;

/// Next to the exe when that directory is writable — which is the case for the
/// portable build people actually run — and under LOCALAPPDATA otherwise, since
/// an installed app cannot write into Program Files.
fn resolve_path() -> PathBuf {
    let beside_exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("llm-desktop.log")));

    if let Some(p) = beside_exe {
        if OpenOptions::new().create(true).append(true).open(&p).is_ok() {
            return p;
        }
    }
    let dir = dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("llm-desktop");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("llm-desktop.log")
}

fn path() -> &'static PathBuf {
    static PATH: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    PATH.get_or_init(resolve_path)
}

fn lock() -> &'static Mutex<()> {
    static LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn stamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string()
}

/// Appends one line. Never returns an error: logging must not be able to break
/// the thing it is logging.
pub fn write(level: &str, source: &str, message: &str) {
    let p = path();
    let Ok(_guard) = lock().lock() else { return };

    // Keep one generation around so a long session cannot fill the disk.
    if std::fs::metadata(p).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
        let _ = std::fs::rename(p, p.with_extension("log.1"));
    }

    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(p) {
        let one_line = message.replace('\n', " ⏎ ");
        let _ = writeln!(f, "{} {:<5} [{}] {}", stamp(), level, source, one_line);
    }
}

pub fn info(source: &str, message: &str) {
    write("INFO", source, message);
}

pub fn warn(source: &str, message: &str) {
    write("WARN", source, message);
}

pub fn error(source: &str, message: &str) {
    write("ERROR", source, message);
}

/// Routes the frontend's own warnings, errors and timings into the same file,
/// so a freeze and the command that caused it sit next to each other.
#[tauri::command]
pub fn append_log(level: String, source: String, message: String) {
    let level = match level.as_str() {
        "error" => "ERROR",
        "warn" => "WARN",
        _ => "INFO",
    };
    write(level, &source, &message);
}

#[tauri::command]
pub fn log_path() -> String {
    path().to_string_lossy().into_owned()
}

/// Opens the log in the system file browser, selecting the file itself.
#[tauri::command]
pub fn reveal_log() -> Result<(), String> {
    let p = path();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("explorer")
            .arg(format!("/select,{}", p.display()))
            .creation_flags(0x0800_0000)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(not(windows))]
    {
        let _ = p;
    }
    Ok(())
}

/// A panic in a worker thread would otherwise vanish silently.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let where_ = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "unknown".into());
        error("panic", &format!("{info} at {where_}"));
        previous(info);
    }));
}
