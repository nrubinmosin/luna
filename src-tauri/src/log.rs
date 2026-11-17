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
    jiff::ZonedDateTime::now().strftime("%Y-%m-%d %H:%M:%S%.3f").to_string()
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
    write_at(&stamp(), level, source, message);
}

/// `write`, stamped `at` (`stamp`): when the line was said, which may be
/// before it is written (`append_log`).
fn write_at(at: &str, level: &str, source: &str, message: &str) {
    let p = path();
    let Ok(_guard) = lock().lock() else { return };
    // Other processes of this Luna (its exit watcher) write here too: the
    // rotation and the append happen under a lock file, so no line is lost to
    // a rename in the middle of it.
    // Each file made private as it is made, and one there that is not this
    // user's alone moved aside (`secret::open_private`): the data dir's pass
    // at start covers only what is there by then, and a luna.log put in its
    // place after a rotation would have every later line.
    let file_lock = crate::secret::open_private(&p.with_extension("log.lock"));
    let _file_guard = file_lock.as_ref().ok().filter(|f| f.lock().is_ok());

    // Keep one generation around so a long session cannot fill the disk.
    if std::fs::metadata(p).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
        let _ = std::fs::rename(p, p.with_extension("log.1"));
    }

    if let Ok(mut f) = crate::secret::append_private(p) {
        // One write for each line: appends from two processes interleave
        // between writes, never inside one.
        let mut put = |at: &str, level: &str, source: &str, message: &str| {
            let one_line = message.replace('\n', " ⏎ ");
            let source = match ROLE.get() {
                Some(role) => format!("{role}/{source}"),
                None => source.to_string(),
            };
            let line = format!("{} {} {:<5} [{}] {}\n", at, env!("CARGO_PKG_VERSION"), level, source, one_line);
            let _ = f.write_all(line.as_bytes());
        };
        put(at, level, source, message);
        // What was moved aside, this file included, said here and not as it
        // happened: the log's own file is opened the same way.
        for note in crate::secret::take_notes() {
            put(&stamp(), "WARN", "app", &note);
        }
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
/// so a freeze and the command that caused it sit next to each other. Handed
/// to a thread of its own (`PAGE`), which writes them in the order they came,
/// each stamped when it came: each line takes the log's lock file and checks
/// the file is private, and on the window's main thread, where a sync command
/// runs, that held the window up for every line. Past `PAGE_MAX` bytes
/// waiting, a line is refused with an error, and the writer says how many
/// were once it has caught up.
#[tauri::command]
pub fn append_log(level: String, source: String, message: String) -> Result<(), String> {
    let level = match level.as_str() {
        "error" => "ERROR",
        "warn" => "WARN",
        _ => "INFO",
    };
    let (tx, backlog) = PAGE.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<PageLine>();
        let backlog = std::sync::Arc::new(PageBacklog::default());
        let theirs = std::sync::Arc::clone(&backlog);
        let _ = std::thread::Builder::new().name("luna-page-log".into()).spawn(move || write_page_lines(rx, &theirs, write_at));
        (tx, backlog)
    });
    hand_over(tx, backlog, PAGE_MAX, Line { at: stamp(), level, source, message })
}

/// How many bytes of the page's lines may wait for their writer.
const PAGE_MAX: u64 = 1024 * 1024;

/// The writer of the page's lines, started with the first, and its backlog.
#[allow(clippy::type_complexity)]
static PAGE: std::sync::OnceLock<(std::sync::mpsc::Sender<PageLine>, std::sync::Arc<PageBacklog>)> = std::sync::OnceLock::new();

/// One of the page's lines, stamped when it came.
struct Line {
    at: String,
    level: &'static str,
    source: String,
    message: String,
}

impl Line {
    fn len(&self) -> u64 {
        (self.source.len() + self.message.len()) as u64
    }
}

enum PageLine {
    Line(Line),
    /// Answered once everything handed over before it is written.
    Flush(std::sync::mpsc::Sender<()>),
}

/// The bytes of the page's lines waiting, and how many were refused since
/// the writer last said so.
#[derive(Default)]
struct PageBacklog {
    bytes: std::sync::atomic::AtomicU64,
    refused: std::sync::atomic::AtomicU64,
}

/// Hands one line to the writer: refused past `max` bytes waiting, written
/// in place where the writer could not be started.
fn hand_over(tx: &std::sync::mpsc::Sender<PageLine>, backlog: &PageBacklog, max: u64, line: Line) -> Result<(), String> {
    use std::sync::atomic::Ordering::SeqCst;
    let len = line.len();
    if backlog.bytes.load(SeqCst) + len > max {
        backlog.refused.fetch_add(1, SeqCst);
        return Err("the log's writer is behind: line not written".into());
    }
    backlog.bytes.fetch_add(len, SeqCst);
    if let Err(std::sync::mpsc::SendError(PageLine::Line(l))) = tx.send(PageLine::Line(line)) {
        backlog.bytes.fetch_sub(len, SeqCst);
        write_at(&l.at, l.level, &l.source, &l.message);
    }
    Ok(())
}

/// The page's lines as they come, each written as `put` writes one, and,
/// after a refusal, a line saying how many were refused.
fn write_page_lines(rx: std::sync::mpsc::Receiver<PageLine>, backlog: &PageBacklog, mut put: impl FnMut(&str, &str, &str, &str)) {
    use std::sync::atomic::Ordering::SeqCst;
    for item in rx {
        match item {
            PageLine::Line(l) => {
                put(&l.at, l.level, &l.source, &l.message);
                backlog.bytes.fetch_sub(l.len(), SeqCst);
                let refused = backlog.refused.swap(0, SeqCst);
                if refused > 0 {
                    put(&stamp(), "WARN", "log", &format!("{refused} of the page's lines were not written: the log's writer was behind"));
                }
            }
            PageLine::Flush(done) => {
                let _ = done.send(());
            }
        }
    }
}

/// Waits, up to `within`, for the page's lines handed over so far to be
/// written: on the way out, so the last of them are not lost with the
/// process. Nothing to wait for where no line has come.
pub fn flush_page_lines(within: std::time::Duration) {
    let Some((tx, _)) = PAGE.get() else { return };
    let (done, wait) = std::sync::mpsc::channel();
    if tx.send(PageLine::Flush(done)).is_ok() {
        let _ = wait.recv_timeout(within);
    }
}

/// A panic in a worker thread would otherwise vanish silently.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let where_ = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_else(|| "unknown".into());
        error("panic", &format!("{info} at {where_}"));
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn line(at: &str, message: &str) -> Line {
        Line { at: at.into(), level: "INFO", source: "page".into(), message: message.into() }
    }

    /// The page's lines are written in the order they came, each with the
    /// time it came; past the bound a line is refused with an error, and the
    /// log says so once the writer moves on; a flush is answered once all
    /// handed over before it is written.
    #[test]
    fn the_pages_lines_keep_their_order_and_time_and_a_refusal_is_told() {
        let (tx, rx) = std::sync::mpsc::channel();
        let backlog = Arc::new(PageBacklog::default());
        let written: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
        let gate = Arc::new(Mutex::new(()));
        let held = gate.lock().unwrap();
        let writer = {
            let (backlog, written, gate) = (Arc::clone(&backlog), Arc::clone(&written), Arc::clone(&gate));
            std::thread::spawn(move || {
                write_page_lines(rx, &backlog, |at, _, _, message| {
                    let _open = gate.lock().unwrap();
                    written.lock().unwrap().push((at.to_string(), message.to_string()));
                })
            })
        };
        // Each line is 4 + 5 bytes: room for two.
        hand_over(&tx, &backlog, 20, line("t1", "first")).unwrap();
        hand_over(&tx, &backlog, 20, line("t2", "secnd")).unwrap();
        assert!(hand_over(&tx, &backlog, 20, line("t3", "third")).unwrap_err().contains("behind"));
        drop(held);
        let (done, wait) = std::sync::mpsc::channel();
        tx.send(PageLine::Flush(done)).unwrap();
        wait.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        let got = written.lock().unwrap().clone();
        assert_eq!(got.len(), 3, "{got:?}");
        assert_eq!(got[0], ("t1".to_string(), "first".to_string()));
        assert!(got[1].1.contains("1 of the page's lines were not written"), "{got:?}");
        assert_eq!(got[2], ("t2".to_string(), "secnd".to_string()));
        drop(tx);
        writer.join().unwrap();
    }
}
