//! One Luna window per data dir: a second launch hands over to the first,
//! which brings its window up, and exits. Tauri's single-instance plugin did
//! this per identifier; here it is per identifier and data dir, which is one
//! core's (overview.md), so the test build (an identifier and a data dir of
//! its own) runs beside the Luna in use, and so does a second copy of Luna
//! unpacked elsewhere.
//!
//! As today (lib.rs `run`): no claim at all under `LUNA_DEV_TOKEN`, a dev
//! build a script drives beside a Luna someone is using; and a test build in
//! the background (`crate::background`) still claims, but a second launch,
//! which is a script's, does not bring its window up.
//!
//! Windows: a named mutex in the session's namespace says who is first, and
//! the first keeps a message-only window, found by its class and title,
//! that takes the second's arguments by WM_COPYDATA. The second lets the
//! first take the foreground (`AllowSetForegroundWindow`), which Windows
//! would otherwise refuse to a window brought up by a process in the
//! background. The window lives on a thread of its own, with its own
//! message loop, so nothing here depends on the window's toolkit.
//!
//! Unix: an exclusive lock on `luna.window.lock` in the data dir says who is
//! first, and the first listens on `luna.window.sock` beside it.
//!
//! The callback runs on that thread, not the window's; it should hand the
//! work over and return. The arguments it gets are the second launch's own,
//! its program name left out.

use std::path::Path;

/// How the claim went.
pub enum Claim {
    /// This is the first: hold the guard for as long as the window runs.
    First(Guard),
    /// Another Luna on this data dir has the arguments now: exit.
    Second,
}

/// Holds the claim, and the listener for later launches, until dropped.
pub struct Guard {
    held: Option<imp::Held>,
}

/// The most a hand-over carries: arguments, not files.
const MAX_PAYLOAD: usize = 64 << 10;

/// Claims the window for this data dir, or hands over to the Luna that has
/// it. `on_second` gets each later launch's arguments.
pub fn claim(on_second: impl Fn(Vec<String>) + Send + Sync + 'static) -> Result<Claim, String> {
    if std::env::var_os("LUNA_DEV_TOKEN").is_some() {
        return Ok(Claim::First(Guard { held: None }));
    }
    let quiet = crate::background();
    let dir = crate::paths::data_dir();
    let args: Vec<String> = std::env::args().skip(1).collect();
    claim_as(&name(super::identifier(), &dir), &dir, &args, move |args| {
        if !quiet {
            on_second(args)
        }
    })
}

/// The name the claim goes by on Windows: the identifier and a hash of the
/// data dir, in the form every spelling of that folder shares.
pub fn name(identifier: &str, data_dir: &Path) -> String {
    use sha2::{Digest, Sha256};
    let key = crate::paths::path_key(&data_dir.to_string_lossy());
    let hash = Sha256::digest(key.as_bytes());
    let hex: String = hash[..8].iter().map(|b| format!("{b:02x}")).collect();
    format!("{identifier}-{hex}")
}

/// What a hand-over carries: the arguments, NUL between them (no argument
/// holds one).
fn encode(args: &[String]) -> Vec<u8> {
    args.join("\0").into_bytes()
}

fn decode(bytes: &[u8]) -> Vec<String> {
    if bytes.is_empty() {
        return Vec::new();
    }
    bytes.split(|&b| b == 0).map(|a| String::from_utf8_lossy(a).into_owned()).collect()
}

/// `claim` with its place given: `name` on Windows, `dir` elsewhere.
fn claim_as(name: &str, dir: &Path, args: &[String], on_second: impl Fn(Vec<String>) + Send + Sync + 'static) -> Result<Claim, String> {
    let callback: Callback = Box::new(move |args| {
        // A panic in the window's callback must not cross the OS's call.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| on_second(args)));
    });
    imp::claim(name, dir, &encode(args), callback)
}

type Callback = Box<dyn Fn(Vec<String>) + Send + Sync>;

#[cfg(windows)]
mod imp {
    use super::{decode, Callback, Claim, Guard, MAX_PAYLOAD};
    use std::path::Path;
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::DataExchange::COPYDATASTRUCT;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::{CreateMutexW, GetCurrentProcessId, GetCurrentThreadId};
    use windows::Win32::UI::WindowsAndMessaging::{
        AllowSetForegroundWindow, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, FindWindowExW, GetMessageW,
        GetWindowLongPtrW, GetWindowThreadProcessId, PostThreadMessageW, RegisterClassExW, SendMessageTimeoutW, SetWindowLongPtrW,
        UnregisterClassW, CREATESTRUCTW, GWLP_USERDATA, HWND_MESSAGE, MSG, SMTO_ABORTIFHUNG, WINDOW_EX_STYLE, WINDOW_STYLE, WM_COPYDATA,
        WM_NCCREATE, WM_QUIT, WNDCLASSEXW,
    };

    /// WM_COPYDATA's dwData for a hand-over: "LUNA".
    const MAGIC: usize = 0x4C55_4E41;

    pub struct Held {
        mutex: HANDLE,
        thread: u32,
        join: Option<std::thread::JoinHandle<()>>,
    }

    // SAFETY: a mutex handle is the process's, usable and closable from any
    // thread; the rest is a thread id and its join handle.
    unsafe impl Send for Held {}

    impl Drop for Held {
        fn drop(&mut self) {
            // SAFETY: the thread's id, from the thread itself; its queue
            // exists since it made its window. The mutex is this guard's.
            unsafe {
                let _ = PostThreadMessageW(self.thread, WM_QUIT, WPARAM(0), LPARAM(0));
            }
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
            // SAFETY: as above.
            unsafe {
                let _ = CloseHandle(self.mutex);
            }
        }
    }

    fn names(name: &str) -> (HSTRING, HSTRING, HSTRING) {
        (HSTRING::from(format!("Local\\{name}-sim")), HSTRING::from(format!("{name}-sic")), HSTRING::from(format!("{name}-siw")))
    }

    pub fn claim(name: &str, _dir: &Path, payload: &[u8], callback: Callback) -> Result<Claim, String> {
        let (mutex_name, class, title) = names(name);
        // SAFETY: a name that outlives the call; the handle is closed by the
        // guard, or right here by the second.
        let mutex = unsafe { CreateMutexW(None, false, &mutex_name) }.map_err(|e| format!("no single-instance mutex: {e}"))?;
        // SAFETY: read right after the call that set it.
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            // SAFETY: the handle just opened.
            unsafe {
                let _ = CloseHandle(mutex);
            }
            hand_over(&class, &title, payload);
            return Ok(Claim::Second);
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name("single-instance".into()).spawn(move || serve(class, title, callback, tx));
        let failed = match spawned {
            Ok(join) => match rx.recv() {
                Ok(Ok(thread)) => return Ok(Claim::First(Guard { held: Some(Held { mutex, thread, join: Some(join) }) })),
                // The thread ends right after saying why: nothing to wait for.
                Ok(Err(e)) => e,
                Err(_) => "the single-instance thread ended".to_string(),
            },
            Err(e) => e.to_string(),
        };
        // SAFETY: the handle just opened.
        unsafe {
            let _ = CloseHandle(mutex);
        }
        Err(failed)
    }

    /// Sends `payload` to the first instance's window. It may not be made yet
    /// when two launches come at once: a second to find it.
    fn hand_over(class: &HSTRING, title: &HSTRING, payload: &[u8]) {
        for _ in 0..40 {
            // SAFETY: names that outlive the call.
            if let Ok(hwnd) = unsafe { FindWindowExW(Some(HWND_MESSAGE), None, class, title) } {
                let mut pid = 0;
                // SAFETY: a window just found; pid is written by the call.
                unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
                // SAFETY: plain calls. Not to this very process (a test's).
                if pid != 0 && pid != unsafe { GetCurrentProcessId() } {
                    let _ = unsafe { AllowSetForegroundWindow(pid) };
                }
                let cds = COPYDATASTRUCT { dwData: MAGIC, cbData: payload.len() as u32, lpData: payload.as_ptr() as *mut _ };
                // SAFETY: the struct and the bytes outlive the call, which
                // returns once the receiver is done with them.
                unsafe {
                    let _ = SendMessageTimeoutW(
                        hwnd,
                        WM_COPYDATA,
                        WPARAM(0),
                        LPARAM(&cds as *const _ as isize),
                        SMTO_ABORTIFHUNG,
                        10_000,
                        None,
                    );
                }
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        crate::log::warn("app", "another Luna holds this data folder, but its window could not be found");
    }

    /// The listener's thread: its window, then its message loop until the
    /// guard posts WM_QUIT.
    fn serve(class: HSTRING, title: HSTRING, callback: Callback, ready: std::sync::mpsc::Sender<Result<u32, String>>) {
        // SAFETY: this thread's own module, class and window; the callback's
        // box is handed to the window and taken back once it is destroyed.
        unsafe {
            let instance = match GetModuleHandleW(PCWSTR::null()) {
                Ok(m) => HINSTANCE(m.0),
                Err(e) => {
                    let _ = ready.send(Err(e.to_string()));
                    return;
                }
            };
            let wc = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(proc_),
                hInstance: instance,
                lpszClassName: PCWSTR(class.as_ptr()),
                ..Default::default()
            };
            // Fails for a class already there (a guard dropped and a new
            // claim made in the same process), which is as good.
            let _ = RegisterClassExW(&wc);
            let boxed = Box::into_raw(Box::new(callback));
            let made = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                &class,
                &title,
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(instance),
                Some(boxed as *const std::ffi::c_void),
            );
            let hwnd = match made {
                Ok(hwnd) => hwnd,
                Err(e) => {
                    drop(Box::from_raw(boxed));
                    let _ = ready.send(Err(format!("no single-instance window: {e}")));
                    return;
                }
            };
            let _ = ready.send(Ok(GetCurrentThreadId()));
            let mut msg = MSG::default();
            // 0 is WM_QUIT, -1 an error: either ends the loop.
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                DispatchMessageW(&msg);
            }
            let _ = DestroyWindow(hwnd);
            drop(Box::from_raw(boxed));
            let _ = UnregisterClassW(&class, Some(instance));
        }
    }

    unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        match msg {
            WM_NCCREATE => {
                // SAFETY: WM_NCCREATE's lParam is the CREATESTRUCTW, whose
                // lpCreateParams is the callback's box (serve).
                unsafe {
                    let create = &*(lparam.0 as *const CREATESTRUCTW);
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
                    DefWindowProcW(hwnd, msg, wparam, lparam)
                }
            }
            WM_COPYDATA => {
                // SAFETY: WM_COPYDATA's lParam is a COPYDATASTRUCT valid for
                // the call; the user data is the callback's box, alive until
                // the window is destroyed.
                unsafe {
                    let cds = &*(lparam.0 as *const COPYDATASTRUCT);
                    let size = cds.cbData as usize;
                    if cds.dwData != MAGIC || size > MAX_PAYLOAD || (size > 0 && cds.lpData.is_null()) {
                        return LRESULT(0);
                    }
                    let bytes = if size == 0 { &[][..] } else { std::slice::from_raw_parts(cds.lpData as *const u8, size) };
                    let callback = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Callback;
                    if let Some(callback) = callback.as_ref() {
                        callback(decode(bytes));
                    }
                    LRESULT(1)
                }
            }
            // SAFETY: passes on, unchanged, a message Windows handed this window.
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }
}

#[cfg(unix)]
mod imp {
    use super::{decode, Callback, Claim, Guard, MAX_PAYLOAD};
    use std::io::{Read, Write};
    use std::os::unix::io::AsRawFd;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    pub struct Held {
        // Holds the lock for as long as it is open.
        _lock: std::fs::File,
        sock: PathBuf,
        stop: Arc<AtomicBool>,
        join: Option<std::thread::JoinHandle<()>>,
    }

    impl Drop for Held {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            // Wakes the accept, which then sees the stop.
            let _ = UnixStream::connect(&self.sock);
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
            let _ = std::fs::remove_file(&self.sock);
        }
    }

    pub fn claim(_name: &str, dir: &Path, payload: &[u8], callback: Callback) -> Result<Claim, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let lock_path = dir.join("luna.window.lock");
        let sock = dir.join("luna.window.sock");
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .map_err(|e| format!("{}: {e}", lock_path.display()))?;
        // SAFETY: a descriptor of a file held open here.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let e = std::io::Error::last_os_error();
            if e.raw_os_error() != Some(libc::EWOULDBLOCK) {
                return Err(format!("{}: {e}", lock_path.display()));
            }
            hand_over(&sock, payload);
            return Ok(Claim::Second);
        }
        // Left by a Luna that did not end cleanly: the lock is ours, so it is
        // nobody's.
        let _ = std::fs::remove_file(&sock);
        let listener = UnixListener::bind(&sock).map_err(|e| format!("{}: {e}", sock.display()))?;
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600));
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let join = std::thread::Builder::new()
            .name("single-instance".into())
            .spawn(move || {
                for conn in listener.incoming() {
                    if stopped.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(conn) = conn else { continue };
                    let _ = conn.set_read_timeout(Some(Duration::from_secs(5)));
                    let mut bytes = Vec::new();
                    if conn.take(MAX_PAYLOAD as u64 + 1).read_to_end(&mut bytes).is_ok() && bytes.len() <= MAX_PAYLOAD {
                        callback(decode(&bytes));
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Claim::First(Guard { held: Some(Held { _lock: lock, sock, stop, join: Some(join) }) }))
    }

    /// Sends `payload` to the first instance. Its socket may not be up yet
    /// when two launches come at once: a second to find it.
    fn hand_over(sock: &Path, payload: &[u8]) {
        for _ in 0..40 {
            if let Ok(mut conn) = UnixStream::connect(sock) {
                let _ = conn.write_all(payload);
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        crate::log::warn("app", "another Luna holds this data folder, but it does not answer");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn the_name_is_one_per_identifier_and_data_folder() {
        let a = name("rm.luna", Path::new("E:/Programs/Luna"));
        assert!(a.starts_with("rm.luna-") && a.len() == "rm.luna-".len() + 16, "{a}");
        assert_ne!(a, name("rm.luna.test", Path::new("E:/Programs/Luna")));
        assert_ne!(a, name("rm.luna", Path::new("D:/Luna")));
        // Windows takes a folder's name in any case and with either slash.
        if cfg!(windows) {
            assert_eq!(a, name("rm.luna", Path::new(r"e:\programs\luna")));
        }
    }

    #[test]
    fn arguments_cross_whole() {
        let args = vec!["--flag".to_string(), "a b".to_string(), "".to_string(), "été".to_string()];
        assert_eq!(decode(&encode(&args)), args);
        assert!(decode(&encode(&[])).is_empty());
    }

    #[test]
    fn a_second_launch_hands_its_arguments_to_the_first_and_the_place_frees_with_it() {
        let tag = format!("luna-single-test-{}", std::process::id());
        let dir = std::env::temp_dir().join(&tag);
        let (tx, rx) = mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        let first = claim_as(&tag, &dir, &[], move |args| {
            let _ = tx.lock().unwrap().send(args);
        })
        .unwrap();
        let Claim::First(guard) = first else { panic!("the first claim was not first") };
        let args = vec!["--hello".to_string(), "two words".to_string()];
        let second = claim_as(&tag, &dir, &args, |_| panic!("the second's callback ran")).unwrap();
        assert!(matches!(second, Claim::Second));
        assert_eq!(rx.recv_timeout(Duration::from_secs(10)).unwrap(), args);
        drop(guard);
        let again = claim_as(&tag, &dir, &[], |_| {}).unwrap();
        assert!(matches!(again, Claim::First(_)), "the place was not freed");
        drop(again);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
