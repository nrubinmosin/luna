//! One Luna window per data dir: a second launch tells the first, which
//! brings its window up, and exits. Tauri's single-instance plugin did this
//! per identifier; here it is per identifier and data dir, which is one
//! core's (overview.md), so the test build (an identifier and a data dir of
//! its own) runs beside the Luna in use, and so does a second copy of Luna
//! unpacked elsewhere.
//!
//! As today (lib.rs `run`): no claim at all under `LUNA_DEV_TOKEN`, a dev
//! build a script drives beside a Luna someone is using; and a test build in
//! the background (`crate::background`) still claims, but a second launch,
//! which is a script's, does not bring its window up.
//!
//! The hand-over is focus only: it carries nothing, and the first does
//! nothing with a second launch but bring its window up. Its listener is not
//! authenticated: on Windows any process on the same desktop at the same
//! integrity or above can find its window and send it the message (Windows
//! keeps a lower integrity's message out, UIPI), and on Unix any process of
//! this user can reach its socket. Such a process can already bring up any
//! of this user's windows, so the listener gives it nothing new; that holds
//! only while the hand-over stays focus only, which is why it carries no
//! arguments, no path, nothing the first would act on.
//!
//! Windows: named mutexes in the session's namespace say who is first, one
//! per key of the data dir (`dir_keys`: where it is, and what it is where
//! the volume can say), all of them the first's; and the first keeps a
//! message-only window per key, found by its class and title, that a second
//! launch which finds that key taken sends a WM_COPYDATA tagged `LUNA`. The
//! keys are claimed in one order, so of two launches at once one wins. The
//! second lets the first take the foreground (`AllowSetForegroundWindow`),
//! which Windows would otherwise refuse to a window brought up by a process
//! in the background. The windows live on a thread of their own, with its
//! own message loop, so nothing here depends on the window's toolkit. A first
//! Luna run as administrator makes a mutex a normal second launch may not
//! open (`ERROR_ACCESS_DENIED`): that one is second too, and its message is
//! kept out by UIPI, which it says in the log before it exits.
//!
//! Unix: an exclusive lock on `window/lock` in the data dir says who is
//! first, and the first listens on `window/sock` beside it; `window/` is a
//! private folder of Luna's (`secret::private_dir`), and both are made there
//! without following a link. The first takes a connection only from this
//! user (`SO_PEERCRED`, `getpeereid`) that says `LUNA` within a second. A
//! second launch's connect and write are held to a second all told, and the
//! first's listener is stopped through a socket pair of its own, so a
//! backlog someone keeps full holds up neither.
//!
//! The callback runs on the listener's thread, not the window's; it should
//! hand the work over and return.

use std::path::Path;
use std::time::{Duration, Instant};

/// How the claim went.
pub enum Claim {
    /// This is the first: hold the guard for as long as the window runs.
    First(Guard),
    /// Another Luna on this data dir has been told to come up: exit.
    Second,
}

/// Holds the claim, and the listener for later launches, until dropped.
pub struct Guard {
    held: Option<imp::Held>,
}

/// What a hand-over says, and all it says.
const MAGIC: &[u8; 4] = b"LUNA";

/// How long a restarted window (`restarting`) goes on trying for the claim
/// of the window it replaces, past `lifecycle::wait_for_previous`'s wait.
const RESTART_WAIT: Duration = Duration::from_secs(15);

/// Claims the window for this data dir, or tells the Luna that has it to
/// come up. `on_second` runs for each later launch. `restarting`: this is the
/// window a tray Restart started (`--after`), and the one holding the claim is
/// the one it replaces, on its way out: it is waited out for `RESTART_WAIT`
/// and never handed this launch, and an error says the restart failed. An
/// error is for the caller to stop on: a window opened without the claim
/// would run beside the one that has it.
pub fn claim(on_second: impl Fn() + Send + Sync + 'static, restarting: bool) -> Result<Claim, String> {
    if std::env::var_os("LUNA_DEV_TOKEN").is_some() {
        return Ok(Claim::First(Guard { held: None }));
    }
    let quiet = crate::background();
    let dir = crate::paths::data_dir();
    // Made here if it is not there yet (a first run in LOCALAPPDATA): where
    // it is and what it is name the claim.
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let names: Vec<String> = dir_keys(&dir)?.iter().map(|key| name(super::identifier(), key)).collect();
    let on_second: std::sync::Arc<dyn Fn() + Send + Sync> = std::sync::Arc::new(move || {
        crate::log::warn(
            "app",
            if quiet {
                "a second launch was handed over; the background window stays where it is"
            } else {
                "a second launch was handed over"
            },
        );
        if !quiet {
            on_second()
        }
    });
    if !restarting {
        return claim_as(&names, &dir, on_second, true);
    }
    let deadline = Instant::now() + RESTART_WAIT;
    loop {
        if let Claim::First(guard) = claim_as(&names, &dir, on_second.clone(), false)? {
            return Ok(Claim::First(guard));
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "Restart window failed: the window it replaces still holds this data folder {}s after it was asked to go",
                (RESTART_WAIT + Duration::from_secs(30)).as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// The name the claim goes by on Windows for one of the data dir's keys
/// (`dir_keys`): the identifier and a hash of the key.
pub fn name(identifier: &str, key: &str) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(key.as_bytes());
    let hex: String = hash[..8].iter().map(|b| format!("{b:02x}")).collect();
    format!("{identifier}-{hex}")
}

/// The keys the claim on the folder `dir` goes by, every one of them held by
/// the Luna that has it. On Windows, where it is: its path as Windows
/// resolves a handle on it, normalized and case-folded (a long name and its
/// 8.3 one, a junction to it, any case: one path); and what it is, its
/// volume's serial number and its file id, where the volume keeps ids for
/// good (`volume::Volume::stable_ids`). The path alone does not do: a data
/// dir deleted and made again under a running Luna is another folder at the
/// same path, which that Luna still uses. The id alone does not either: FAT
/// moves ids, a share may make them up or give none, and a failure there
/// must not keep Luna from starting. On Unix its device and inode.
pub fn dir_keys(dir: &Path) -> Result<Vec<String>, String> {
    let failed = |e: std::io::Error| format!("{} cannot be told apart from another folder: {e}", dir.display());
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS;
        // A link at its name followed: the folder it leads to is the one.
        let file = std::fs::OpenOptions::new().read(true).custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0).open(dir).map_err(failed)?;
        let path = match super::volume::final_path(&file) {
            Ok(path) => path,
            // Then the path as given, made absolute: still one spelling per
            // name, case aside.
            Err(e) => {
                crate::log::warn("app", &format!("{}: no final path ({e}); the single instance goes by the path as given", dir.display()));
                std::path::absolute(dir).map_err(failed)?.to_string_lossy().into_owned()
            }
        };
        let identity = match super::volume::of(&file, &path) {
            Ok(volume) if volume.stable_ids() => file_id(&file),
            Ok(_) => None,
            Err(e) => {
                crate::log::warn(
                    "app",
                    &format!("{}: its volume does not say what it keeps ({e}); the single instance goes by its path", dir.display()),
                );
                None
            }
        };
        Ok(keys(identity, &path))
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::metadata(dir).map_err(failed)?;
        Ok(vec![format!("{:x}:{:x}", meta.dev(), meta.ino())])
    }
    #[cfg(not(any(windows, unix)))]
    {
        Ok(vec![crate::paths::path_key(&dir.to_string_lossy())])
    }
}

/// The keys for a folder at `path` (as `volume::final_path` gives it) and
/// of `identity` where it has one for good: the identity first, so that two
/// launches claim in the same order.
fn keys(identity: Option<String>, path: &str) -> Vec<String> {
    let mut keys: Vec<String> = identity.into_iter().map(|id| format!("id:{id}")).collect();
    keys.push(format!("path:{}", path.to_uppercase()));
    keys
}

/// The folder's volume serial number and 128-bit file id; None where the
/// volume gives none.
#[cfg(windows)]
fn file_id(file: &std::fs::File) -> Option<String> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{FileIdInfo, GetFileInformationByHandleEx, FILE_ID_INFO};
    let mut id = FILE_ID_INFO::default();
    // SAFETY: the handle is the File's, open through the call; the buffer is
    // of the size the call is told.
    let got = unsafe {
        GetFileInformationByHandleEx(HANDLE(file.as_raw_handle()), FileIdInfo, (&raw mut id).cast(), size_of::<FILE_ID_INFO>() as u32)
    };
    if got.is_err() || id.FileId.Identifier.iter().all(|b| *b == 0) {
        return None;
    }
    let hex: String = id.FileId.Identifier.iter().map(|b| format!("{b:02x}")).collect();
    Some(format!("{:016x}:{hex}", id.VolumeSerialNumber))
}

/// `claim` with its place given: `names` on Windows, `dir` elsewhere. With
/// `hand_over`, a second launch tells the first; without, it only finds it.
fn claim_as(names: &[String], dir: &Path, on_second: std::sync::Arc<dyn Fn() + Send + Sync>, hand_over: bool) -> Result<Claim, String> {
    let callback: Callback = Box::new(move || {
        // A panic in the window's callback must not cross the OS's call.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(&*on_second));
    });
    imp::claim(names, dir, callback, hand_over)
}

type Callback = Box<dyn Fn() + Send + Sync>;

#[cfg(windows)]
mod imp {
    use super::{Callback, Claim, Guard, MAGIC};
    use std::path::Path;
    use windows::core::{HRESULT, HSTRING, PCWSTR};
    use windows::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM,
    };
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
    const TAG: usize = u32::from_be_bytes(*MAGIC) as usize;

    pub struct Held {
        mutexes: Vec<HANDLE>,
        thread: u32,
        join: Option<std::thread::JoinHandle<()>>,
    }

    // SAFETY: a mutex handle is the process's, usable and closable from any
    // thread; the rest is a thread id and its join handle.
    unsafe impl Send for Held {}

    impl Drop for Held {
        fn drop(&mut self) {
            // SAFETY: the thread's id, from the thread itself; its queue
            // exists since it made its windows.
            unsafe {
                let _ = PostThreadMessageW(self.thread, WM_QUIT, WPARAM(0), LPARAM(0));
            }
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
            close(&self.mutexes);
        }
    }

    /// Closes mutex handles this module opened.
    fn close(mutexes: &[HANDLE]) {
        for mutex in mutexes {
            // SAFETY: a handle opened here, closed once.
            unsafe {
                let _ = CloseHandle(*mutex);
            }
        }
    }

    fn names(name: &str) -> (HSTRING, HSTRING, HSTRING) {
        (HSTRING::from(format!("Local\\{name}-sim")), HSTRING::from(format!("{name}-sic")), HSTRING::from(format!("{name}-siw")))
    }

    /// First only with every name's mutex; second at the first that is
    /// taken, which goes to the window of that name.
    pub fn claim(claimed: &[String], _dir: &Path, callback: Callback, tell: bool) -> Result<Claim, String> {
        let mut mutexes = Vec::new();
        let mut windows = Vec::new();
        for name in claimed {
            let (mutex_name, class, title) = names(name);
            // SAFETY: a name that outlives the call; the handle is closed by
            // the guard, or here once the claim is lost.
            let mutex = match unsafe { CreateMutexW(None, false, &mutex_name) } {
                Ok(mutex) => mutex,
                // There, made by a Luna run as administrator, whose mutex a
                // normal run may not open: that one is first.
                Err(e) if e.code() == HRESULT::from_win32(ERROR_ACCESS_DENIED.0) => {
                    close(&mutexes);
                    if tell {
                        hand_over(&class, &title);
                    }
                    return Ok(Claim::Second);
                }
                Err(e) => {
                    close(&mutexes);
                    return Err(format!("no single-instance mutex: {e}"));
                }
            };
            // SAFETY: read right after the call that set it.
            let taken = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
            mutexes.push(mutex);
            if taken {
                close(&mutexes);
                if tell {
                    hand_over(&class, &title);
                }
                return Ok(Claim::Second);
            }
            windows.push((class, title));
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name("single-instance".into()).spawn(move || serve(windows, callback, tx));
        let failed = match spawned {
            Ok(join) => match rx.recv() {
                Ok(Ok(thread)) => return Ok(Claim::First(Guard { held: Some(Held { mutexes, thread, join: Some(join) }) })),
                // The thread ends right after saying why: nothing to wait for.
                Ok(Err(e)) => e,
                Err(_) => "the single-instance thread ended".to_string(),
            },
            Err(e) => e.to_string(),
        };
        close(&mutexes);
        Err(failed)
    }

    /// Tells the first instance's window to come up, best effort. It may not
    /// be made yet when two launches come at once: a second to find it.
    fn hand_over(class: &HSTRING, title: &HSTRING) {
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
                let cds = COPYDATASTRUCT { dwData: TAG, cbData: 0, lpData: std::ptr::null_mut() };
                // SAFETY: the struct outlives the call, which returns once
                // the receiver is done with it. The error is read right after.
                let (sent, error) = unsafe {
                    let sent = SendMessageTimeoutW(
                        hwnd,
                        WM_COPYDATA,
                        WPARAM(0),
                        LPARAM(&cds as *const _ as isize),
                        SMTO_ABORTIFHUNG,
                        10_000,
                        None,
                    );
                    (sent, GetLastError())
                };
                if sent.0 == 0 {
                    // A first run as administrator (UIPI keeps the message
                    // out), a hung or a timed-out one.
                    let why = std::io::Error::from_raw_os_error(error.0 as i32);
                    crate::log::warn("app", &format!("another Luna holds this data folder, but did not take the hand-over: {why}"));
                }
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        crate::log::warn("app", "another Luna holds this data folder, but its window could not be found");
    }

    /// The listener's thread: a window per name, then its message loop until
    /// the guard posts WM_QUIT.
    fn serve(windows: Vec<(HSTRING, HSTRING)>, callback: Callback, ready: std::sync::mpsc::Sender<Result<u32, String>>) {
        // SAFETY: this thread's own module, classes and windows; the
        // callback's box is handed to the windows and taken back once every
        // one of them is destroyed.
        unsafe {
            let instance = match GetModuleHandleW(PCWSTR::null()) {
                Ok(m) => HINSTANCE(m.0),
                Err(e) => {
                    let _ = ready.send(Err(e.to_string()));
                    return;
                }
            };
            let boxed = Box::into_raw(Box::new(callback));
            let mut made = Vec::new();
            let mut failed = None;
            for (class, title) in &windows {
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
                let window = CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    class,
                    title,
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
                match window {
                    Ok(hwnd) => made.push(hwnd),
                    Err(e) => {
                        failed = Some(format!("no single-instance window: {e}"));
                        break;
                    }
                }
            }
            match failed {
                Some(e) => {
                    let _ = ready.send(Err(e));
                }
                None => {
                    let _ = ready.send(Ok(GetCurrentThreadId()));
                    let mut msg = MSG::default();
                    // 0 is WM_QUIT, -1 an error: either ends the loop.
                    while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                        DispatchMessageW(&msg);
                    }
                }
            }
            for hwnd in made {
                let _ = DestroyWindow(hwnd);
            }
            drop(Box::from_raw(boxed));
            for (class, _) in &windows {
                let _ = UnregisterClassW(class, Some(instance));
            }
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
                // the call, of which only the tag is read; the user data is
                // the callback's box, alive until the window is destroyed.
                unsafe {
                    let cds = &*(lparam.0 as *const COPYDATASTRUCT);
                    if cds.dwData != TAG {
                        return LRESULT(0);
                    }
                    let callback = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Callback;
                    if let Some(callback) = callback.as_ref() {
                        callback();
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
    use super::{Callback, Claim, Guard, MAGIC};
    use std::io::{Read, Write};
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    use std::os::unix::io::{AsRawFd, RawFd};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    /// How long a connection has to say `MAGIC`, all told.
    const SAY_WITHIN: Duration = Duration::from_secs(1);

    /// How long a second launch looks for the first's listener, connecting
    /// included: a first still starting, or one whose backlog is full.
    const HAND_OVER_WITHIN: Duration = Duration::from_secs(1);

    pub struct Held {
        // Holds the lock for as long as it is open.
        _lock: std::fs::File,
        sock: PathBuf,
        // The listener's thread waits on the other end of this alongside the
        // socket: dropped, it stops the thread, however full the backlog.
        stop: Option<UnixStream>,
        join: Option<std::thread::JoinHandle<()>>,
    }

    impl Drop for Held {
        fn drop(&mut self) {
            drop(self.stop.take());
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
            let _ = std::fs::remove_file(&self.sock);
        }
    }

    pub fn claim(_names: &[String], dir: &Path, callback: Callback, tell: bool) -> Result<Claim, String> {
        // The data dir's own folder for the two, this user's alone: no link,
        // nobody else's, 0700 (made so, or refused).
        let place = dir.join("window");
        crate::secret::private_dir(&place).map_err(|e| format!("{}: {e}", place.display()))?;
        let lock_path = place.join("lock");
        let sock = place.join("sock");
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&lock_path)
            .map_err(|e| format!("{}: {e}", lock_path.display()))?;
        // SAFETY: a descriptor of a file held open here.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let e = std::io::Error::last_os_error();
            if e.raw_os_error() != Some(libc::EWOULDBLOCK) {
                return Err(format!("{}: {e}", lock_path.display()));
            }
            if tell {
                hand_over(&sock);
            }
            return Ok(Claim::Second);
        }
        // Left by a Luna that did not end cleanly: the lock is ours, so it is
        // nobody's. A link there goes as itself.
        let _ = std::fs::remove_file(&sock);
        let listener = UnixListener::bind(&sock).map_err(|e| format!("{}: {e}", sock.display()))?;
        // Just made in a folder only this user can change, so the name is
        // still the socket's.
        std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600)).map_err(|e| format!("{}: {e}", sock.display()))?;
        listener.set_nonblocking(true).map_err(|e| format!("{}: {e}", sock.display()))?;
        let (stop, stopped) = UnixStream::pair().map_err(|e| e.to_string())?;
        let join = std::thread::Builder::new()
            .name("single-instance".into())
            .spawn(move || serve(&listener, &stopped, &callback))
            .map_err(|e| e.to_string())?;
        Ok(Claim::First(Guard { held: Some(Held { _lock: lock, sock, stop: Some(stop), join: Some(join) }) }))
    }

    /// The listener's thread: takes each connection as it comes until
    /// `stopped`'s other end goes (`Held`'s drop), which it waits on beside
    /// the listener, so the stop needs no connection of its own.
    fn serve(listener: &UnixListener, stopped: &UnixStream, callback: &Callback) {
        loop {
            let mut fds = [
                libc::pollfd { fd: listener.as_raw_fd(), events: libc::POLLIN, revents: 0 },
                libc::pollfd { fd: stopped.as_raw_fd(), events: libc::POLLIN, revents: 0 },
            ];
            // SAFETY: two pollfds, as counted, of descriptors held here.
            let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) };
            if ready < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return;
            }
            // Its other end gone (or written to): stop.
            if fds[1].revents != 0 {
                return;
            }
            if fds[0].revents & (libc::POLLERR | libc::POLLNVAL) != 0 {
                return;
            }
            // Nonblocking: one that went before it was taken is no wait. Out
            // of descriptors or the like, a pause rather than a spin.
            let mut conn = match listener.accept() {
                Ok((conn, _)) => conn,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
                Err(_) => {
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                }
            };
            // A BSD gives the accepted socket the listener's O_NONBLOCK.
            if conn.set_nonblocking(false).is_ok() && same_user(conn.as_raw_fd()) && says_magic(&mut conn, Instant::now() + SAY_WITHIN) {
                callback();
            }
        }
    }

    /// Whether the process at the other end of `fd` runs as this user.
    fn same_user(fd: RawFd) -> bool {
        // SAFETY: geteuid has no preconditions and cannot fail.
        let me = unsafe { libc::geteuid() };
        #[cfg(target_os = "linux")]
        {
            crate::endpoint::peer_cred(fd).is_ok_and(|cred| cred.uid == me)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let (mut uid, mut gid) = (0, 0);
            // SAFETY: a socket of this process; both out-parameters are written once.
            unsafe { libc::getpeereid(fd, &mut uid, &mut gid) == 0 && uid == me }
        }
    }

    /// Whether `conn` says `MAGIC` before `deadline`: one deadline for the
    /// whole read, however the bytes come.
    fn says_magic(conn: &mut UnixStream, deadline: Instant) -> bool {
        let mut got = [0u8; 4];
        let mut have = 0;
        while have < got.len() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || conn.set_read_timeout(Some(left)).is_err() {
                return false;
            }
            match conn.read(&mut got[have..]) {
                Ok(0) => return false,
                Ok(n) => have += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => return false,
            }
        }
        got == *MAGIC
    }

    /// Tells the first instance to come up, within `HAND_OVER_WITHIN` all
    /// told. Its socket may not be up yet when two launches come at once,
    /// and a connect to a listener whose backlog is full would wait for
    /// good: each try is held to what is left of the time
    /// (`endpoint::connect_within`), and so is the write: what is left of
    /// it once connected.
    fn hand_over(sock: &Path) {
        let deadline = Instant::now() + HAND_OVER_WITHIN;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            if let Ok(mut conn) = crate::endpoint::connect_within(sock, left) {
                // A zero timeout is refused (EINVAL): the deadline passed while
                // connecting is a timed-out write.
                let left = deadline.saturating_duration_since(Instant::now()).max(Duration::from_millis(1));
                let written = conn.set_write_timeout(Some(left)).and_then(|()| conn.write_all(MAGIC));
                if let Err(e) = written {
                    crate::log::warn("app", &format!("another Luna holds this data folder, but did not take the hand-over: {e}"));
                }
                return;
            }
            std::thread::sleep(left.min(Duration::from_millis(25)));
        }
        crate::log::warn("app", "another Luna holds this data folder, but it does not answer");
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_connection_must_say_the_word_within_one_deadline() {
            let (mut a, mut b) = UnixStream::pair().unwrap();
            a.write_all(MAGIC).unwrap();
            assert!(says_magic(&mut b, Instant::now() + SAY_WITHIN));
            a.write_all(b"LUNX").unwrap();
            assert!(!says_magic(&mut b, Instant::now() + SAY_WITHIN));
            // A byte at a time, each in time for a per-read timeout, but not
            // all four before the one deadline.
            let writer = std::thread::spawn(move || {
                for byte in MAGIC {
                    std::thread::sleep(Duration::from_millis(150));
                    let _ = a.write_all(&[*byte]);
                }
            });
            assert!(!says_magic(&mut b, Instant::now() + Duration::from_millis(400)));
            writer.join().unwrap();
            assert!(same_user(b.as_raw_fd()));
        }

        /// A first whose backlog is full (connections nobody takes) neither
        /// holds a second launch past its deadline nor its own stop: the
        /// stop is a socket pair of its own, not a connection.
        #[test]
        fn a_full_backlog_holds_up_neither_a_second_launch_nor_the_stop() {
            let root = std::env::temp_dir().join(format!("luna-single-backlog-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            let sock = root.join("sock");
            // A listener that takes nothing, backlog of one.
            let listener = UnixListener::bind(&sock).unwrap();
            // SAFETY: the listener's own descriptor.
            assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 1) }, 0);
            let mut filled = Vec::new();
            for _ in 0..64 {
                match crate::endpoint::connect_within(&sock, Duration::from_millis(50)) {
                    Ok(conn) => filled.push(conn),
                    Err(_) => break,
                }
            }
            let started = Instant::now();
            hand_over(&sock);
            assert!(started.elapsed() < HAND_OVER_WITHIN + Duration::from_millis(500), "{:?}", started.elapsed());
            drop((listener, filled));

            // A first of its own, filled the same way, stops at once.
            let first = claim(&[], &root, Box::new(|| {}), true).unwrap();
            let Claim::First(guard) = first else { panic!("not first") };
            let mut held = Vec::new();
            for _ in 0..64 {
                match crate::endpoint::connect_within(&root.join("window").join("sock"), Duration::from_millis(50)) {
                    Ok(conn) => held.push(conn),
                    Err(_) => break,
                }
            }
            let started = Instant::now();
            drop(guard);
            // Each connection held may take its second to say nothing.
            assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
            drop(held);
            std::fs::remove_dir_all(&root).unwrap();
        }

        #[test]
        fn a_link_in_place_of_the_folder_is_refused() {
            let root = std::env::temp_dir().join(format!("luna-single-link-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("elsewhere")).unwrap();
            std::os::unix::fs::symlink(root.join("elsewhere"), root.join("window")).unwrap();
            assert!(claim(&[], &root, Box::new(|| {}), true).is_err());
            assert_eq!(std::fs::read_dir(root.join("elsewhere")).unwrap().count(), 0);
            std::fs::remove_dir_all(&root).unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{mpsc, Arc};

    #[test]
    fn the_name_is_one_per_identifier_and_folder() {
        let a = name("rm.luna", "00c0ffee:0123");
        assert!(a.starts_with("rm.luna-") && a.len() == "rm.luna-".len() + 16, "{a}");
        assert_ne!(a, name("rm.luna.test", "00c0ffee:0123"));
        assert_ne!(a, name("rm.luna", "00c0ffee:0124"));
    }

    /// One folder gives one key whatever names it, another folder another:
    /// its name in another case, by a junction or symlink, and on Windows its
    /// 8.3 name, where the volume keeps those.
    #[test]
    fn every_name_of_a_folder_gives_it_one_key() {
        let base = std::env::temp_dir().join(format!("luna-single-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let long = base.join("a rather long folder name");
        std::fs::create_dir_all(&long).unwrap();
        std::fs::create_dir_all(base.join("other")).unwrap();
        let key = dir_keys(&long).unwrap();
        assert_ne!(key, dir_keys(&base.join("other")).unwrap());
        assert_eq!(key, dir_keys(&long.join(".")).unwrap());
        assert!(dir_keys(&base.join("missing")).is_err());
        #[cfg(windows)]
        {
            use windows::core::HSTRING;
            use windows::Win32::Storage::FileSystem::GetShortPathNameW;
            let upper = std::path::PathBuf::from(long.to_string_lossy().to_ascii_uppercase());
            assert_eq!(key, dir_keys(&upper).unwrap());
            let mut short = vec![0u16; 1024];
            // SAFETY: a path that outlives the call, and a buffer of the length passed.
            let n = unsafe { GetShortPathNameW(&HSTRING::from(long.as_os_str()), Some(&mut short)) } as usize;
            let short = std::path::PathBuf::from(String::from_utf16(&short[..n]).unwrap());
            if n > 0 && short != long {
                assert_eq!(key, dir_keys(&short).unwrap(), "{}", short.display());
            }
            let made =
                std::process::Command::new("cmd").args(["/d", "/c", "mklink", "/J"]).arg(base.join("link")).arg(&long).output().unwrap();
            assert!(made.status.success());
            assert_eq!(key, dir_keys(&base.join("link")).unwrap());
            std::fs::remove_dir(base.join("link")).unwrap();
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&long, base.join("link")).unwrap();
            assert_eq!(key, dir_keys(&base.join("link")).unwrap());
        }
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// A folder whose volume keeps ids for good goes by what it is and by
    /// where it is; one whose volume does not (FAT, a share), or that gives
    /// no id, by where it is alone. The path in any case is one key.
    #[test]
    fn the_keys_are_the_identity_and_the_path_or_the_path_alone() {
        let both = keys(Some("00c0ffee:0123".into()), r"\\?\E:\Luna");
        assert_eq!(both, [r"id:00c0ffee:0123", r"path:\\?\E:\LUNA"]);
        assert_eq!(keys(None, r"\\?\E:\Luna"), [r"path:\\?\E:\LUNA"]);
        assert_eq!(keys(None, r"\\?\e:\luna"), keys(None, r"\\?\E:\LUNA"));
        assert_ne!(keys(None, r"\\?\E:\Luna"), keys(None, r"\\?\E:\Luna2"));
    }

    /// A data dir deleted and made again at the same path is another folder
    /// (another id) at the same place: the path key still finds the Luna
    /// using it.
    #[cfg(windows)]
    #[test]
    fn a_folder_made_again_keeps_its_path_key() {
        let dir = std::env::temp_dir().join(format!("luna-single-again-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let before = dir_keys(&dir).unwrap();
        assert_eq!(before.len(), 2, "a local NTFS folder goes by both: {before:?}");
        // Another folder in between, so the id is not handed straight back.
        let other = dir.with_extension("other");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::remove_dir(&dir).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let after = dir_keys(&dir).unwrap();
        assert_eq!(before[1], after[1], "one path");
        assert_ne!(before[0], after[0], "another folder");
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&other).unwrap();
    }

    /// A launch that shares one name with the first, either one, is second,
    /// and its hand-over reaches the first through that name's window.
    #[cfg(windows)]
    #[test]
    fn one_name_in_common_with_the_first_makes_a_second() {
        let tag = format!("luna-single-names-{}", std::process::id());
        let [id, path, other_id, other_path] = ["id", "path", "id2", "path2"].map(|k| format!("{tag}-{k}"));
        let dir = std::env::temp_dir();
        let (tx, rx) = mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        let first = claim_as(
            &[id.clone(), path.clone()],
            &dir,
            Arc::new(move || {
                let _ = tx.lock().unwrap().send(());
            }),
            true,
        )
        .unwrap();
        let Claim::First(guard) = first else { panic!("the first claim was not first") };
        for names in [[other_id.clone(), path.clone()], [id.clone(), other_path.clone()]] {
            let second = claim_as(&names, &dir, Arc::new(|| panic!("the second's callback ran")), true).unwrap();
            assert!(matches!(second, Claim::Second), "{names:?}");
            rx.recv_timeout(Duration::from_secs(10)).unwrap();
        }
        // The names a second held on the way are let go with it.
        let third = claim_as(&[other_id, other_path], &dir, Arc::new(|| {}), true).unwrap();
        assert!(matches!(third, Claim::First(_)), "a second kept a name it lost the claim with");
        drop((third, guard));
    }

    #[test]
    fn a_second_launch_brings_the_first_up_and_the_place_frees_with_it() {
        let tag = format!("luna-single-test-{}", std::process::id());
        let dir = std::env::temp_dir().join(&tag);
        let (tx, rx) = mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        let first = claim_as(
            std::slice::from_ref(&tag),
            &dir,
            Arc::new(move || {
                let _ = tx.lock().unwrap().send(());
            }),
            true,
        )
        .unwrap();
        let Claim::First(guard) = first else { panic!("the first claim was not first") };
        // A restarted window only finds it, and tells it nothing.
        let quiet = claim_as(std::slice::from_ref(&tag), &dir, Arc::new(|| panic!("the second's callback ran")), false).unwrap();
        assert!(matches!(quiet, Claim::Second));
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "told without a hand-over");
        let second = claim_as(std::slice::from_ref(&tag), &dir, Arc::new(|| panic!("the second's callback ran")), true).unwrap();
        assert!(matches!(second, Claim::Second));
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err(), "one launch, one call");
        drop(guard);
        let again = claim_as(std::slice::from_ref(&tag), &dir, Arc::new(|| {}), true).unwrap();
        assert!(matches!(again, Claim::First(_)), "the place was not freed");
        drop(again);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
