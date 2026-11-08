//! No CLI runs with administrator rights.
//!
//! A process started as administrator ("Run as administrator", an elevated
//! terminal) hands its elevated token to everything it starts, and the CLIs
//! would run with it: every tool command an agent runs, with the whole machine
//! in reach. So the elevated token is kept from them three ways:
//!
//! - The window, finding itself elevated, starts the core with the token the
//!   person's ordinary programs run with: the desktop shell's (`plan`,
//!   `spawn_as_shell`). Where there is no such token of the same user, it
//!   starts no core and says why.
//! - The core, and any process that would start a CLI, checks its own token
//!   (`refusal`, `guard`): an elevated one starts no session or helper,
//!   whatever asks, so a de-elevation that failed, or a core started elevated
//!   by hand, still runs no CLI.
//! - A running core found elevated (one from before this check) is not
//!   worked with (corelink.rs): replaced when it has no session, the person
//!   asked to restart it when it has.
//!
//! Off Windows root is refused the same way; there is no ordinary token to
//! go back to there that Luna could know to be the person's.
//!
//! Why the shell's token and not the others Windows offers. The elevated
//! token's linked token (`TokenLinkedToken`) is the right one in principle,
//! but a caller without SeTcbPrivilege gets it back at identification level
//! only, which no process can be started with. The shell's `IShellDispatch2`
//! (Explorer starting the program for us) gives no handle and no say over the
//! environment. A scheduled task with least privilege leaves a task behind
//! and is stopped by Task Scheduler's own time limit. The shell's token,
//! duplicated and used with `CreateProcessWithTokenW`, is Microsoft's own
//! answer (Aaron Margosis, "FAQ: How do I start a program as the desktop user
//! from an elevated app?") and Chromium's (`base/win/elevation_util.cc`,
//! `RunDeElevated`); it takes SeImpersonatePrivilege, which an elevated admin
//! holds, and the Secondary Logon service.

/// What the window and the core say when they run as administrator.
pub const ADMIN: &str = "Luna is running as administrator; restart it normally.";
/// The same off Windows. `sudo -E` keeps the caller's environment, HOME with
/// it: the CLIs would run as root on the person's own account folders.
pub const ROOT: &str = "Luna is running as root; restart it as your own user. Under sudo -E it would keep your \
    HOME, and the CLIs would run as root on your own account folders and leave files there only root can change.";

/// What the OS says of one process's token, as far as this module cares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    /// The user: a SID as text on Windows, a uid elsewhere.
    pub user: String,
    /// Administrator rights in effect: an elevated token, or one above medium
    /// integrity (SYSTEM's among them), on Windows; uid 0 elsewhere.
    pub elevated: bool,
    /// The elevated half of a split admin's token, a limited one linked to it
    /// (`TokenElevationTypeFull`). Without one — User Account Control off, the
    /// built-in Administrator — the account has no ordinary token at all.
    pub split: bool,
}

/// The OS, as the decisions below see it; tests stand in for it.
pub trait Os {
    /// This process's token. None if it cannot be read.
    fn me(&self) -> Option<Token>;
    /// The desktop shell's process and its token, when there is one Luna can read.
    fn shell(&self) -> Option<(u32, Token)>;
    /// Whether this OS has an ordinary token to go back to (Windows).
    fn de_elevates(&self) -> bool;
}

/// How the window starts a core.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Launch {
    /// As itself: it is not elevated.
    Directly,
    /// With the token of the desktop shell, this pid.
    AsShell(u32),
    Refused(Refusal),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// This process's own token could not be read.
    Unknown,
    /// Root, off Windows.
    Root,
    /// No desktop shell to take an ordinary token from.
    NoShell,
    /// The shell is another user's: elevated with someone else's credentials.
    OtherUser,
    /// The shell is elevated too: the account has no limited token.
    NoLimitedToken,
}

impl Refusal {
    pub fn message(self) -> String {
        let why = match self {
            Refusal::Root => return ROOT.into(),
            Refusal::Unknown => "Luna cannot tell whether it runs as administrator, so it starts no session.",
            Refusal::NoShell => {
                "It starts no session: the desktop shell (Explorer) is not running, and its token is \
                the one sessions are started with when Luna runs as administrator."
            }
            Refusal::OtherUser => {
                "It was started with the credentials of another account than the one signed in to \
                this desktop: it starts no session as that account, and none as yours unasked."
            }
            Refusal::NoLimitedToken => {
                "This account has no standard-user token to start sessions with (User Account \
                Control is off, or this is the built-in Administrator)."
            }
        };
        format!("{ADMIN} {why}")
    }
}

/// How a window running with `os`'s token starts a core.
pub fn plan(os: &impl Os) -> Launch {
    let Some(me) = os.me() else { return Launch::Refused(Refusal::Unknown) };
    if !me.elevated {
        return Launch::Directly;
    }
    if !os.de_elevates() {
        return Launch::Refused(Refusal::Root);
    }
    let Some((pid, shell)) = os.shell() else {
        return Launch::Refused(if me.split { Refusal::NoShell } else { Refusal::NoLimitedToken });
    };
    // Over-the-shoulder elevation: the desktop is the person's, the token
    // someone else's. Neither is the one to start sessions as.
    if shell.user != me.user {
        return Launch::Refused(Refusal::OtherUser);
    }
    if shell.elevated {
        return Launch::Refused(Refusal::NoLimitedToken);
    }
    Launch::AsShell(pid)
}

/// Why a process running with `os`'s token must start no CLI, if it must.
pub fn refusal_of(os: &impl Os) -> Option<String> {
    match os.me() {
        Some(t) if !t.elevated => None,
        Some(_) if os.de_elevates() => Some(ADMIN.into()),
        Some(_) => Some(ROOT.into()),
        None => Some(Refusal::Unknown.message()),
    }
}

/// The OS's own answer for this process. Read once; a token's elevation does
/// not change under a running process.
fn os_refusal() -> Option<&'static str> {
    static R: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    R.get_or_init(|| refusal_of(&System)).as_deref()
}

/// Why this process starts no CLI: it runs with administrator rights.
#[cfg(not(test))]
pub fn refusal() -> Option<&'static str> {
    os_refusal()
}

/// In the tests too the OS's own answer comes first: the live suite
/// (live.rs) starts real CLIs from a test executable, and one run from an
/// elevated terminal must be refused like Luna. A test may add a refusal
/// (`refuse_here`), never lift one; a test that only builds a launch without
/// running it is let past by its own capture (pty/spawn.rs).
#[cfg(test)]
pub fn refusal() -> Option<&'static str> {
    os_refusal().or_else(|| REFUSED_HERE.with(std::cell::Cell::get))
}

#[cfg(test)]
thread_local! {
    static REFUSED_HERE: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
}

/// For this test thread: `refusal` says `why` as well as whatever the OS
/// says; None takes it back.
#[cfg(test)]
pub fn refuse_here(why: Option<&'static str>) {
    REFUSED_HERE.with(|f| f.set(why));
}

/// Where a CLI is about to be started: refused in a process that runs as
/// administrator.
pub fn guard() -> Result<(), String> {
    refusal().map_or(Ok(()), |why| Err(why.to_string()))
}

/// The OS itself.
pub struct System;

#[cfg(windows)]
impl Os for System {
    fn me(&self) -> Option<Token> {
        win::own_token().ok()
    }
    fn shell(&self) -> Option<(u32, Token)> {
        win::shell()
    }
    fn de_elevates(&self) -> bool {
        true
    }
}

#[cfg(unix)]
impl Os for System {
    fn me(&self) -> Option<Token> {
        // SAFETY: geteuid has no preconditions and cannot fail.
        let uid = unsafe { libc::geteuid() };
        Some(Token { user: uid.to_string(), elevated: uid == 0, split: false })
    }
    fn shell(&self) -> Option<(u32, Token)> {
        None
    }
    fn de_elevates(&self) -> bool {
        false
    }
}

/// Whether the process `pid` runs with rights above an ordinary user's.
/// None when that cannot be told (it is gone, or not to be opened at all).
#[cfg(windows)]
pub fn process_elevated(pid: u32) -> Option<bool> {
    win::process_elevated(pid)
}

/// Root's: its effective uid is 0. Linux says so in `/proc/<pid>/status`;
/// macOS has no /proc, and `ps` reads it from the kernel there.
#[cfg(unix)]
pub fn process_elevated(pid: u32) -> Option<bool> {
    let from_proc = || std::fs::read_to_string(format!("/proc/{pid}/status")).ok().and_then(|s| euid_in_status(&s));
    let from_ps = || {
        let out = std::process::Command::new("/bin/ps")
            .args(["-o", "uid=", "-p", &pid.to_string()])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        euid_from_ps(&String::from_utf8_lossy(&out.stdout))
    };
    from_proc().or_else(from_ps).map(|uid| uid == 0)
}

/// The effective uid in a `/proc/<pid>/status`: its `Uid:` line holds the
/// real, effective, saved and filesystem uids, in that order.
#[cfg_attr(not(unix), allow(dead_code))]
fn euid_in_status(status: &str) -> Option<u32> {
    status.lines().find_map(|l| l.strip_prefix("Uid:"))?.split_whitespace().nth(1)?.parse().ok()
}

/// `ps -o uid=`: the effective uid alone, padded; nothing for a pid gone.
#[cfg_attr(not(unix), allow(dead_code))]
fn euid_from_ps(out: &str) -> Option<u32> {
    out.trim().parse().ok()
}

/// Starts `cmd` with the token of the desktop shell `pid`, which is checked
/// again here: still this user's, still not elevated.
#[cfg(windows)]
pub fn spawn_as_shell(pid: u32, cmd: &std::process::Command) -> std::io::Result<()> {
    win::spawn_as_shell(pid, cmd)
}

#[cfg(not(windows))]
pub fn spawn_as_shell(_pid: u32, _cmd: &std::process::Command) -> std::io::Result<()> {
    Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "no shell token off Windows"))
}

#[cfg(windows)]
mod win {
    use super::Token;
    use std::ffi::{OsStr, OsString};
    use std::io;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows::core::{BOOL, PCWSTR, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, LocalFree, ERROR_ACCESS_DENIED, HANDLE, HLOCAL};
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows::Win32::Security::{
        DuplicateTokenEx, GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, SecurityImpersonation, TokenElevation,
        TokenElevationType, TokenElevationTypeFull, TokenIntegrityLevel, TokenPrimary, TokenUser, PSID, TOKEN_ACCESS_MASK,
        TOKEN_ADJUST_DEFAULT, TOKEN_ADJUST_SESSIONID, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_ELEVATION, TOKEN_ELEVATION_TYPE,
        TOKEN_INFORMATION_CLASS, TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TOKEN_USER,
    };
    use windows::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
    use windows::Win32::System::JobObjects::{
        IsProcessInJob, JobObjectBasicProcessIdList, QueryInformationJobObject, JOBOBJECT_BASIC_PROCESS_ID_LIST,
    };
    use windows::Win32::System::Threading::{
        CreateProcessWithTokenW, GetCurrentProcess, OpenProcess, OpenProcessToken, ResumeThread, TerminateProcess,
        CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CREATE_PROCESS_LOGON_FLAGS, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT,
        PROCESS_ACCESS_RIGHTS, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, PROCESS_QUERY_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION,
        STARTUPINFOW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetShellWindow, GetWindowThreadProcessId};

    /// `SECURITY_MANDATORY_HIGH_RID`, S-1-16-12288: an elevated admin's level,
    /// below SYSTEM's. Spelled out rather than taken from a `windows` feature
    /// for one constant.
    const HIGH_INTEGRITY: u32 = 0x3000;

    /// A handle closed when dropped.
    struct Owned(HANDLE);

    impl Drop for Owned {
        fn drop(&mut self) {
            // SAFETY: a handle this module opened and nothing else closes.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    fn open_process(pid: u32, access: PROCESS_ACCESS_RIGHTS) -> io::Result<Owned> {
        // SAFETY: no pointers; the handle is owned from here.
        Ok(Owned(unsafe { OpenProcess(access, false, pid) }?))
    }

    fn open_token(process: HANDLE, access: TOKEN_ACCESS_MASK) -> windows::core::Result<Owned> {
        let mut token = HANDLE::default();
        // SAFETY: `token` receives a handle owned from here.
        unsafe { OpenProcessToken(process, access, &mut token) }?;
        Ok(Owned(token))
    }

    /// `class` of `token`, in a buffer of u64s so the struct it holds is aligned.
    fn info(token: HANDLE, class: TOKEN_INFORMATION_CLASS) -> io::Result<Vec<u64>> {
        let mut len = 0u32;
        // SAFETY: a size query; nothing is written but `len`.
        let _ = unsafe { GetTokenInformation(token, class, None, 0, &mut len) };
        if len == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        // SAFETY: the buffer holds `len` bytes.
        unsafe { GetTokenInformation(token, class, Some(buf.as_mut_ptr().cast()), len, &mut len) }?;
        Ok(buf)
    }

    fn sid_text(sid: PSID) -> io::Result<String> {
        let mut text = PWSTR::null();
        // SAFETY: `sid` points into a buffer the caller keeps alive; the
        // string made is LocalAlloc'd, read here and freed once.
        unsafe {
            ConvertSidToStringSidW(sid, &mut text)?;
            let out = text.to_string();
            let _ = LocalFree(Some(HLOCAL(text.0.cast())));
            out.map_err(io::Error::other)
        }
    }

    fn read(token: HANDLE) -> io::Result<Token> {
        let user = info(token, TokenUser)?;
        // SAFETY: TokenUser fills the buffer with a TOKEN_USER, its SID in it.
        let user = sid_text(unsafe { (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid })?;
        let elevation = info(token, TokenElevation)?;
        // SAFETY: TokenElevation fills the buffer with a TOKEN_ELEVATION.
        let elevated = unsafe { (*elevation.as_ptr().cast::<TOKEN_ELEVATION>()).TokenIsElevated } != 0;
        let kind = info(token, TokenElevationType)?;
        // SAFETY: TokenElevationType fills the buffer with a TOKEN_ELEVATION_TYPE.
        let split = unsafe { *kind.as_ptr().cast::<TOKEN_ELEVATION_TYPE>() } == TokenElevationTypeFull;
        let label = info(token, TokenIntegrityLevel)?;
        // SAFETY: TokenIntegrityLevel fills the buffer with a TOKEN_MANDATORY_LABEL
        // whose SID is S-1-16-<level>: the level is its last sub-authority.
        let level = unsafe {
            let sid = (*label.as_ptr().cast::<TOKEN_MANDATORY_LABEL>()).Label.Sid;
            let count = u32::from(*GetSidSubAuthorityCount(sid));
            *GetSidSubAuthority(sid, count.saturating_sub(1))
        };
        Ok(Token { user, elevated: elevated || level >= HIGH_INTEGRITY, split })
    }

    pub fn own_token() -> io::Result<Token> {
        // SAFETY: the current process's pseudo-handle.
        let token = open_token(unsafe { GetCurrentProcess() }, TOKEN_QUERY)?;
        read(token.0)
    }

    /// The process behind the desktop's shell window, of this window station:
    /// Explorer, unless the shell was replaced. None with no shell running.
    fn shell_pid() -> Option<u32> {
        // SAFETY: no arguments; a null window is checked for.
        let window = unsafe { GetShellWindow() };
        if window.is_invalid() {
            return None;
        }
        let mut pid = 0u32;
        // SAFETY: `pid` is written once.
        unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
        (pid != 0).then_some(pid)
    }

    pub fn shell() -> Option<(u32, Token)> {
        let pid = shell_pid()?;
        let process = open_process(pid, PROCESS_QUERY_LIMITED_INFORMATION).ok()?;
        let token = open_token(process.0, TOKEN_QUERY).ok()?;
        Some((pid, read(token.0).ok()?))
    }

    /// A process that may not be opened, or whose token may not be read, runs
    /// above this one: an elevated process's object and token are not open
    /// to the same user's ordinary token (or belongs to another user, which
    /// is no core to work with either).
    pub fn process_elevated(pid: u32) -> Option<bool> {
        let denied = |e: &windows::core::Error| e.code() == windows::core::HRESULT::from_win32(ERROR_ACCESS_DENIED.0);
        // SAFETY: no pointers; the handle is owned from here.
        let process = match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) } {
            Ok(h) => Owned(h),
            Err(e) => return denied(&e).then_some(true),
        };
        match open_token(process.0, TOKEN_QUERY) {
            Ok(token) => read(token.0).ok().map(|t| t.elevated),
            Err(e) => denied(&e).then_some(true),
        }
    }

    pub fn spawn_as_shell(pid: u32, cmd: &std::process::Command) -> io::Result<()> {
        let me = own_token()?;
        let process = open_process(pid, PROCESS_QUERY_INFORMATION)?;
        let shell = open_token(process.0, TOKEN_DUPLICATE | TOKEN_QUERY)?;
        // The token about to be used, not the one `plan` looked at: the shell
        // may have been restarted, its pid taken, in between.
        let seen = read(shell.0)?;
        if seen.user != me.user || seen.elevated {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the desktop shell's token is no longer an ordinary one of this user",
            ));
        }
        let mut primary = HANDLE::default();
        // The access Margosis's sample asks for: what CreateProcessWithTokenW
        // needs of a primary token.
        let access = TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY | TOKEN_DUPLICATE | TOKEN_ADJUST_DEFAULT | TOKEN_ADJUST_SESSIONID;
        // SAFETY: `primary` receives a handle owned from here.
        unsafe { DuplicateTokenEx(shell.0, access, None, SecurityImpersonation, TokenPrimary, &mut primary) }?;
        let primary = Owned(primary);

        let program = wide(cmd.get_program());
        let mut line = command_line(std::iter::once(cmd.get_program()).chain(cmd.get_args()));
        let cwd = cmd.get_current_dir().map(|d| wide(d.as_os_str()));
        let env = core_environment(profile_environment(primary.0)?, std::env::vars_os());
        let startup = STARTUPINFOW { cb: size_of::<STARTUPINFOW>() as u32, ..Default::default() };
        // Suspended, so nothing of it runs before its token is seen to be what
        // was asked for. No handle is inherited: this function has no way to.
        // Out of any job the window was started in, as `spawn_detached` asks;
        // a job that forbids breaking away refuses the first try.
        let flags = CREATE_NEW_PROCESS_GROUP | CREATE_UNICODE_ENVIRONMENT | CREATE_SUSPENDED;
        let create = |flags: PROCESS_CREATION_FLAGS, line: &mut Vec<u16>| {
            let mut info = PROCESS_INFORMATION::default();
            // SAFETY: every buffer is NUL-terminated and outlives the call; the
            // command line is a mutable copy, as the call requires.
            unsafe {
                CreateProcessWithTokenW(
                    primary.0,
                    CREATE_PROCESS_LOGON_FLAGS(0),
                    PCWSTR(program.as_ptr()),
                    Some(PWSTR(line.as_mut_ptr())),
                    flags,
                    Some(env.as_ptr().cast()),
                    cwd.as_ref().map_or(PCWSTR::null(), |d| PCWSTR(d.as_ptr())),
                    &startup,
                    &mut info,
                )
            }
            .map(|()| info)
        };
        let info = create(flags | CREATE_BREAKAWAY_FROM_JOB, &mut line).or_else(|_| create(flags, &mut line))?;
        let (child, thread) = (Owned(info.hProcess), Owned(info.hThread));
        // What became of the job is not taken on trust: Secondary Logon, not
        // this window, creates the process. Said in the log, the one place a
        // run as administrator leaves a trace of it.
        match in_my_job(info.dwProcessId) {
            Some(true) => crate::log::warn(
                "core",
                "the core started with the shell's token is in this window's job: it ends if that job is closed with it",
            ),
            Some(false) => crate::log::info("core", "the core started with the shell's token is outside this window's job"),
            None => crate::log::warn("core", "could not tell whether the core started with the shell's token is in this window's job"),
        }
        let started = open_token(child.0, TOKEN_QUERY).map_err(io::Error::from).and_then(|t| read(t.0));
        if !matches!(&started, Ok(t) if !t.elevated && t.user == me.user) {
            // SAFETY: the handle CreateProcessWithTokenW returned, full access.
            let _ = unsafe { TerminateProcess(child.0, 1) };
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "the process started with the shell's token was elevated"));
        }
        // SAFETY: the primary thread's handle, suspended above.
        if unsafe { ResumeThread(thread.0) } == u32::MAX {
            let e = io::Error::last_os_error();
            // SAFETY: as above.
            let _ = unsafe { TerminateProcess(child.0, 1) };
            return Err(e);
        }
        Ok(())
    }

    fn wide(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(Some(0)).collect()
    }

    /// Whether `pid` is in the job this process runs in: false when this one
    /// is in none; None when the job cannot be read.
    pub(super) fn in_my_job(pid: u32) -> Option<bool> {
        let mut mine = BOOL(0);
        // SAFETY: the current process's pseudo-handle; `mine` is written once.
        unsafe { IsProcessInJob(GetCurrentProcess(), None, &mut mine) }.ok()?;
        if !mine.as_bool() {
            return Some(false);
        }
        let mut cap: usize = 256;
        loop {
            // Two u32 counts, then the pids as usize: the struct's own layout.
            let mut buf = vec![0usize; 1 + cap];
            let bytes = (buf.len() * size_of::<usize>()) as u32;
            // SAFETY: no job handle names this process's own job; the buffer
            // is `bytes` long and read only after the call filled it.
            let got = unsafe { QueryInformationJobObject(None, JobObjectBasicProcessIdList, buf.as_mut_ptr().cast(), bytes, None) };
            // SAFETY: the buffer starts with the JOBOBJECT_BASIC_PROCESS_ID_LIST header.
            let head = unsafe { &*buf.as_ptr().cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>() };
            let (assigned, listed) = (head.NumberOfAssignedProcesses as usize, head.NumberOfProcessIdsInList as usize);
            if got.is_ok() {
                return Some(buf[1..=listed.min(cap)].contains(&(pid as usize)));
            }
            if assigned > cap && cap < 1 << 16 {
                cap = assigned + 64;
                continue;
            }
            return None;
        }
    }

    /// The environment Windows makes for `token`'s user from their profile,
    /// as Explorer's own programs get it: not the window's, whose PATH, TEMP
    /// and the rest an elevated start may have pointed at an administrator's
    /// places.
    fn profile_environment(token: HANDLE) -> io::Result<Vec<(OsString, OsString)>> {
        let mut block: *mut std::ffi::c_void = std::ptr::null_mut();
        // SAFETY: a primary token with the TOKEN_QUERY and TOKEN_DUPLICATE
        // the call needs; not inheriting this process's own variables.
        unsafe { CreateEnvironmentBlock(&mut block, Some(token), false) }?;
        let mut vars = Vec::new();
        // SAFETY: the block is `NAME=value` strings, each NUL-terminated, the
        // last followed by another NUL; read up to that and freed once.
        unsafe {
            let mut at = block.cast::<u16>().cast_const();
            loop {
                let len = (0..).take_while(|&i| *at.add(i) != 0).count();
                if len == 0 {
                    break;
                }
                let entry = std::slice::from_raw_parts(at, len);
                // From the second character: `=C:=C:\dir` names a drive's
                // current folder, its name starting with `=`.
                if let Some(eq) = entry.iter().skip(1).position(|&c| c == u16::from(b'=')) {
                    vars.push((OsString::from_wide(&entry[..eq + 1]), OsString::from_wide(&entry[eq + 2..])));
                }
                at = at.add(len + 1);
            }
            let _ = DestroyEnvironmentBlock(block);
        }
        Ok(vars)
    }

    /// The core's environment: the user's own from their profile, and on top
    /// of it only Luna's own variables (`LUNA_*`) from the window's, which a
    /// test build is started with (the dev caller's token, folder and
    /// account). As a block, sorted by name as Windows expects one.
    pub(super) fn core_environment(profile: Vec<(OsString, OsString)>, window: impl Iterator<Item = (OsString, OsString)>) -> Vec<u16> {
        let key = |k: &OsString| k.to_string_lossy().to_uppercase();
        let mut vars: std::collections::BTreeMap<String, (OsString, OsString)> = profile.into_iter().map(|kv| (key(&kv.0), kv)).collect();
        vars.extend(window.filter(|(k, _)| key(k).starts_with("LUNA_")).map(|kv| (key(&kv.0), kv)));
        let mut block = Vec::new();
        for (k, v) in vars.into_values() {
            block.extend(k.encode_wide());
            block.push(u16::from(b'='));
            block.extend(v.encode_wide());
            block.push(0);
        }
        if block.is_empty() {
            block.push(0);
        }
        block.push(0);
        block
    }

    /// For a test: this process's own profile environment, as the core would
    /// get it with this process's token.
    #[cfg(test)]
    pub(super) fn own_profile_environment() -> io::Result<Vec<(OsString, OsString)>> {
        // SAFETY: the current process's pseudo-handle.
        let token = open_token(unsafe { GetCurrentProcess() }, TOKEN_QUERY | TOKEN_DUPLICATE)?;
        profile_environment(token.0)
    }

    /// `args` as one command line that CommandLineToArgvW and the C runtime
    /// split back into the same arguments, NUL-terminated.
    pub(super) fn command_line<'a>(args: impl Iterator<Item = &'a OsStr>) -> Vec<u16> {
        let (quote, slash, space, tab) = (u16::from(b'"'), u16::from(b'\\'), u16::from(b' '), u16::from(b'\t'));
        let mut out = Vec::new();
        for (i, arg) in args.enumerate() {
            if i > 0 {
                out.push(space);
            }
            let arg: Vec<u16> = arg.encode_wide().collect();
            if !arg.is_empty() && !arg.iter().any(|&c| c == space || c == tab || c == quote) {
                out.extend(arg);
                continue;
            }
            out.push(quote);
            let mut slashes = 0;
            for c in arg {
                if c == slash {
                    slashes += 1;
                } else {
                    // Backslashes before a quote are escaped, and the quote.
                    if c == quote {
                        out.extend(std::iter::repeat_n(slash, slashes + 1));
                    }
                    slashes = 0;
                }
                out.push(c);
            }
            // And before the closing quote.
            out.extend(std::iter::repeat_n(slash, slashes));
            out.push(quote);
        }
        out.push(0);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An OS of the test's making.
    struct Fake {
        me: Option<Token>,
        shell: Option<(u32, Token)>,
        windows: bool,
    }

    impl Os for Fake {
        fn me(&self) -> Option<Token> {
            self.me.clone()
        }
        fn shell(&self) -> Option<(u32, Token)> {
            self.shell.clone()
        }
        fn de_elevates(&self) -> bool {
            self.windows
        }
    }

    fn token(user: &str, elevated: bool, split: bool) -> Token {
        Token { user: user.into(), elevated, split }
    }

    const ME: &str = "S-1-5-21-1-2-3-1001";
    const SOMEONE: &str = "S-1-5-21-1-2-3-1002";

    fn windows(me: Token, shell: Option<Token>) -> Fake {
        Fake { me: Some(me), shell: shell.map(|t| (4242, t)), windows: true }
    }

    #[test]
    fn a_window_that_is_not_elevated_starts_the_core_itself() {
        let os = windows(token(ME, false, false), Some(token(ME, false, false)));
        assert_eq!(plan(&os), Launch::Directly);
        assert_eq!(refusal_of(&os), None);
    }

    #[test]
    fn an_elevated_window_starts_the_core_with_the_shells_token() {
        // A split admin, elevated by consent: the shell runs with the limited
        // half of the same user's token.
        let os = windows(token(ME, true, true), Some(token(ME, false, false)));
        assert_eq!(plan(&os), Launch::AsShell(4242));
        // And a process that is elevated starts no CLI itself, de-elevated or not.
        assert_eq!(refusal_of(&os).as_deref(), Some(ADMIN));
    }

    #[test]
    fn with_no_limited_token_nothing_is_started() {
        // User Account Control off, or the built-in Administrator: the shell is
        // as elevated as the window.
        let os = windows(token(ME, true, false), Some(token(ME, true, false)));
        assert_eq!(plan(&os), Launch::Refused(Refusal::NoLimitedToken));
        let os = windows(token(ME, true, false), None);
        assert_eq!(plan(&os), Launch::Refused(Refusal::NoLimitedToken));
    }

    #[test]
    fn with_no_shell_nothing_is_started() {
        let os = windows(token(ME, true, true), None);
        assert_eq!(plan(&os), Launch::Refused(Refusal::NoShell));
    }

    #[test]
    fn another_users_shell_is_never_borrowed() {
        // Elevated with another account's credentials over the shoulder: the
        // desktop, and its shell, are the signed-in person's.
        let os = windows(token(SOMEONE, true, true), Some(token(ME, false, false)));
        assert_eq!(plan(&os), Launch::Refused(Refusal::OtherUser));
        assert!(Refusal::OtherUser.message().starts_with(ADMIN));
    }

    #[test]
    fn root_is_refused_off_windows() {
        let os = Fake { me: Some(token("0", true, false)), shell: None, windows: false };
        assert_eq!(plan(&os), Launch::Refused(Refusal::Root));
        assert_eq!(refusal_of(&os).as_deref(), Some(ROOT));
        let os = Fake { me: Some(token("1000", false, false)), shell: None, windows: false };
        assert_eq!(plan(&os), Launch::Directly);
        assert_eq!(refusal_of(&os), None);
    }

    #[test]
    fn a_token_that_cannot_be_read_counts_as_elevated() {
        let os = Fake { me: None, shell: None, windows: true };
        assert_eq!(plan(&os), Launch::Refused(Refusal::Unknown));
        assert!(refusal_of(&os).is_some());
    }

    #[test]
    fn the_guard_says_why() {
        refuse_here(Some(ADMIN));
        let refused = guard();
        refuse_here(None);
        assert_eq!(refused, Err(os_refusal().unwrap_or(ADMIN).to_string()));
    }

    /// The live suite's executable runs elevated when its terminal does: the
    /// tests answer as Luna does then, and no test can say otherwise. Here
    /// that is the OS's answer itself, whatever this machine's token is; CI's
    /// Windows runners (elevated) and its Linux containers (root) see the
    /// refusal, a desktop's ordinary terminal does not.
    #[test]
    fn a_test_run_as_administrator_is_refused_like_luna() {
        let os = refusal_of(&System);
        assert_eq!(refusal().map(String::from), os);
        assert_eq!(guard().is_err(), os.is_some());
        assert_eq!(os.is_some(), System.me().is_none_or(|t| t.elevated));
        // Taking a test's own refusal back leaves the OS's in place.
        refuse_here(Some("a test's"));
        refuse_here(None);
        assert_eq!(refusal().map(String::from), os);
    }

    /// What this machine's own token reads as: the tests run elevated on CI's
    /// Windows runners and not here, so only that it reads at all.
    #[cfg(windows)]
    #[test]
    fn this_processs_token_reads() {
        let me = System.me().expect("own token");
        assert!(me.user.starts_with("S-1-5-"), "{me:?}");
        assert_eq!(process_elevated(std::process::id()), Some(me.elevated));
        // SYSTEM's own process: never taken for an ordinary one, whether this
        // token may open it (an admin's) or not.
        assert_ne!(process_elevated(4), Some(false));
        // Where there is a shell (a desktop session, not a service's), it is
        // the signed-in user's, ordinarily this one.
        if let Some((pid, shell)) = System.shell() {
            assert!(pid != 0 && shell.user.starts_with("S-1-5-"), "{shell:?}");
        }
    }

    #[test]
    fn a_processs_effective_uid_is_read_as_its_kernel_gives_it() {
        let status = "Name:\tluna-core\nUid:\t1000\t0\t0\t0\nGid:\t1000\t1000\t1000\t1000\n";
        assert_eq!(euid_in_status(status), Some(0), "the second field, not the real uid");
        assert_eq!(euid_in_status("Name:\tx\n"), None);
        assert_eq!(euid_from_ps("  501\n"), Some(501));
        assert_eq!(euid_from_ps(""), None, "a pid that is gone");
    }

    #[cfg(unix)]
    #[test]
    fn this_processs_uid_reads() {
        // SAFETY: geteuid has no preconditions and cannot fail.
        let root = unsafe { libc::geteuid() } == 0;
        assert_eq!(process_elevated(std::process::id()), Some(root));
    }

    #[cfg(windows)]
    #[test]
    fn the_core_gets_the_users_own_environment_and_lunas_variables_alone() {
        use std::ffi::OsString;
        let os = |s: &str| OsString::from(s);
        let profile = vec![(os("Path"), os(r"C:\Windows")), (os("TEMP"), os(r"C:\Users\me\Temp")), (os("LUNA_DEV_FOLDER"), os("old"))];
        let window = vec![
            (os("PATH"), os(r"C:\admin-only")),
            (os("TEMP"), os(r"C:\Windows\Temp")),
            (os("LUNA_DEV_TOKEN"), os("t0k3n")),
            (os("LUNA_DEV_FOLDER"), os("new")),
        ];
        let block = win::core_environment(profile, window.into_iter());
        assert_eq!(block[block.len() - 2..], [0, 0], "double-NUL terminated");
        let text = String::from_utf16(&block[..block.len() - 2]).unwrap();
        let vars: Vec<&str> = text.split('\0').collect();
        assert_eq!(vars, [r"LUNA_DEV_FOLDER=new", "LUNA_DEV_TOKEN=t0k3n", r"Path=C:\Windows", r"TEMP=C:\Users\me\Temp"]);
    }

    /// The profile's environment as Windows makes it for this token, and the
    /// job check, on this machine.
    #[cfg(windows)]
    #[test]
    fn the_profile_environment_and_the_job_read() {
        let vars = win::own_profile_environment().expect("CreateEnvironmentBlock");
        let has = |name: &str| vars.iter().any(|(k, v)| k.eq_ignore_ascii_case(name) && !v.is_empty());
        assert!(has("USERPROFILE") && has("SystemRoot") && has("Path"), "{vars:?}");
        // This test process is in a job or not; either way it is in its own,
        // and SYSTEM's process is never in it.
        let mut mine = windows::core::BOOL(0);
        // SAFETY: the current process's pseudo-handle; `mine` written once.
        unsafe {
            windows::Win32::System::JobObjects::IsProcessInJob(windows::Win32::System::Threading::GetCurrentProcess(), None, &mut mine)
        }
        .unwrap();
        assert_eq!(win::in_my_job(std::process::id()), Some(mine.as_bool()));
        assert_ne!(win::in_my_job(4), Some(true));
    }

    #[cfg(windows)]
    #[test]
    fn a_command_line_splits_back_into_its_arguments() {
        use std::ffi::OsStr;
        let line = |args: &[&str]| {
            let wide = win::command_line(args.iter().map(OsStr::new));
            String::from_utf16(&wide[..wide.len() - 1]).unwrap()
        };
        assert_eq!(line(&[r"C:\Luna\luna.exe", "--core"]), r"C:\Luna\luna.exe --core");
        assert_eq!(line(&[r"C:\Program Files\Luna\luna.exe", ""]), r#""C:\Program Files\Luna\luna.exe" """#);
        assert_eq!(line(&[r"C:\a dir\", r#"say "hi""#]), r#""C:\a dir\\" "say \"hi\"""#);
    }
}
