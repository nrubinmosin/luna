use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

const MAX_BYTES: u64 = 4 * 1024 * 1024;

/// In the data dir, with everything else of Luna's.
fn resolve_path() -> PathBuf {
    let dir = crate::paths::data_dir();
    let _ = std::fs::create_dir_all(&dir);
    dir.join("luna.log")
}

static PATH: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
static ROLE: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();

pub fn path() -> &'static PathBuf {
    PATH.get_or_init(resolve_path)
}

/// For a process that runs from somewhere other than the app's own folder —
/// the core from its copy, the exit watcher — and writes to the app's log all
/// the same. Before the first line, or it has no effect.
pub fn set_path(p: PathBuf) {
    let _ = PATH.set(p);
}

/// Tags every line this process writes, so two processes sharing the file
/// can be told apart: `[core/pty]` against the window's `[pty]`.
pub fn set_role(role: &'static str) {
    let _ = ROLE.set(role);
}

fn lock() -> &'static Mutex<()> {
    static LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn stamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string()
}

/// Appends one line. Never returns an error: logging must not be able to break
/// the thing it is logging. The file exists for post-mortems, so INFO goes in
/// too: a session's start, exit and deletion, each MCP call, the awake hold —
/// what a night is read back from once the sessions involved are gone. It is
/// events, never chatter on a timer and never what is said in a chat, so the
/// two generations hold weeks. Every line carries the version that wrote it:
/// the file outlives updates, and a warning means little without knowing
/// which build it came from.
pub fn write(level: &str, source: &str, message: &str) {
    let p = path();
    let Ok(_guard) = lock().lock() else { return };
    // Other processes of this Luna (its exit watcher) write here too: the
    // rotation and the append happen under a lock file, so no line is lost to
    // a rename in the middle of it.
    let file_lock = OpenOptions::new().create(true).truncate(false).write(true).open(p.with_extension("log.lock"));
    let _file_guard = file_lock.as_ref().ok().filter(|f| f.lock().is_ok());

    // Keep one generation around so a long session cannot fill the disk.
    if std::fs::metadata(p).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
        let _ = std::fs::rename(p, p.with_extension("log.1"));
    }

    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(p) {
        let one_line = message.replace('\n', " ⏎ ");
        // One write for the whole line: appends from two processes interleave
        // between writes, never inside one.
        let source = match ROLE.get() {
            Some(role) => format!("{role}/{source}"),
            None => source.to_string(),
        };
        let line = format!("{} {} {:<5} [{}] {}\n", stamp(), env!("CARGO_PKG_VERSION"), level, source, one_line);
        let _ = f.write_all(line.as_bytes());
    }
}

/// The line that opens every run, so a restart — and an update with it — is
/// plain in the file. Written whatever the level filter says.
pub fn start(message: &str) {
    write("START", "app", message);
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
