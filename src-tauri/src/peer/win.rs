//! The peer check's facts on Windows: the token a pipe's client opened it with.

use super::{CoreFacts, PeerFacts};
use crate::endpoint::Stream;
use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, IsTokenRestricted, RevertToSelf, TokenIntegrityLevel,
    TokenIsAppContainer, TokenUser, TOKEN_INFORMATION_CLASS, TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TOKEN_USER,
};
use windows::Win32::System::Pipes::{GetNamedPipeClientProcessId, ImpersonateNamedPipeClient};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, OpenProcess, OpenProcessToken, OpenThreadToken, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};

/// A handle closed when dropped.
struct Owned(HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: a handle this module opened and nothing else closes.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// The core's own: its integrity, from its process token. None when it cannot
/// be read, on which every connection is refused. Not cached: a stale None
/// would refuse every connection for the core's life, and the read is a
/// couple of local calls.
pub(super) fn core_facts(refuses_work: bool) -> Option<CoreFacts> {
    let token = own_token()?;
    // SAFETY: a token opened for query, alive for the call.
    let integrity = unsafe { token_integrity(token.0) }?;
    Some(CoreFacts { integrity, no_new_privs: false, refuses_work })
}

fn own_token() -> Option<Owned> {
    let mut token = HANDLE::default();
    // SAFETY: the current process's pseudo-handle; the token is owned from here.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.ok()?;
    Some(Owned(token))
}

/// What the client of the pipe `stream` opened it as, or None when any of it
/// cannot be read (on which `verdict` refuses). The core takes on the
/// client's identity for the one call that opens its token and drops it at
/// once: nothing else runs as the client, and the identification-level token
/// a window grants could do nothing else anyway.
pub(super) fn peer_facts(stream: &Stream) -> Option<PeerFacts> {
    let pipe = stream.raw();
    let mut pid = 0u32;
    // SAFETY: the server end of a connected pipe; `pid` is written once.
    unsafe { GetNamedPipeClientProcessId(pipe, &mut pid) }.ok()?;
    let token = client_token(pipe)?;
    let me = own_token()?;
    // SAFETY: two tokens opened for query, alive for these calls.
    unsafe {
        let same_user = user_sid(token.0)? == user_sid(me.0)?;
        // `IsTokenRestricted` cannot say "unknown" apart from "no": it does
        // not fail for a token just opened.
        let restricted = IsTokenRestricted(token.0).is_ok();
        let app_container = token_is_app_container(token.0)?;
        let integrity = token_integrity(token.0)?;
        Some(PeerFacts {
            pid,
            image: image_name(pid),
            same_user,
            restricted,
            app_container,
            integrity,
            foreign_userns: false,
            no_new_privs: false,
        })
    }
}

/// The client's token, read while impersonating it. The thread is back to the
/// core's own before this returns, whatever came of the read; a thread that
/// could not go back must not run on as the client, and is ended.
fn client_token(pipe: HANDLE) -> Option<Owned> {
    // SAFETY: the server end of a connected pipe.
    unsafe { ImpersonateNamedPipeClient(pipe) }.ok()?;
    let mut token = HANDLE::default();
    // SAFETY: this thread's pseudo-handle; opened as the core itself
    // (`OpenAsSelf`), since an identification-level token cannot open itself.
    // An anonymous client's token cannot be opened at all: None, refused.
    let opened = unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, true, &mut token) };
    // SAFETY: no arguments; ends this thread's impersonation.
    if unsafe { RevertToSelf() }.is_err() {
        panic!("could not stop impersonating a client of the core's endpoint");
    }
    opened.ok()?;
    Some(Owned(token))
}

/// `class` of `token`, in a buffer of u64s so the struct it holds is aligned.
unsafe fn info(token: HANDLE, class: TOKEN_INFORMATION_CLASS) -> Option<Vec<u64>> {
    let mut len = 0u32;
    // SAFETY: a size query, then a read into a buffer of that size.
    unsafe {
        let _ = GetTokenInformation(token, class, None, 0, &mut len);
        if len == 0 {
            return None;
        }
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        GetTokenInformation(token, class, Some(buf.as_mut_ptr().cast()), len, &mut len).ok()?;
        Some(buf)
    }
}

/// The token's user, as a SID string.
unsafe fn user_sid(token: HANDLE) -> Option<String> {
    // SAFETY: TokenUser fills the buffer with a TOKEN_USER, its SID inside
    // the same buffer; the string made is LocalAlloc'd, read and freed here.
    unsafe {
        let buf = info(token, TokenUser)?;
        let sid = (*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid;
        let mut text = windows::core::PWSTR::null();
        ConvertSidToStringSidW(sid, &mut text).ok()?;
        let out = text.to_string().ok();
        let _ = LocalFree(Some(HLOCAL(text.0.cast())));
        out
    }
}

/// Whether `token` is an AppContainer. None when the query fails, so the
/// peer is refused rather than taken for not contained.
unsafe fn token_is_app_container(token: HANDLE) -> Option<bool> {
    let mut is = 0u32;
    let mut len = 0u32;
    // SAFETY: a u32 out-parameter sized to match.
    unsafe { GetTokenInformation(token, TokenIsAppContainer, Some((&mut is as *mut u32).cast()), size_of::<u32>() as u32, &mut len) }
        .ok()?;
    Some(is != 0)
}

/// The mandatory-label RID of `token`: the last sub-authority of its
/// integrity SID.
unsafe fn token_integrity(token: HANDLE) -> Option<u32> {
    // SAFETY: TokenIntegrityLevel fills the buffer with a TOKEN_MANDATORY_LABEL
    // whose SID lives in the same buffer, read while it does.
    unsafe {
        let buf = info(token, TokenIntegrityLevel)?;
        let sid = (*buf.as_ptr().cast::<TOKEN_MANDATORY_LABEL>()).Label.Sid;
        let count = *GetSidSubAuthorityCount(sid);
        (count > 0).then(|| *GetSidSubAuthority(sid, u32::from(count - 1)))
    }
}

/// `pid`'s image file name (its last path component), empty when it cannot
/// be read: it is for the log only.
fn image_name(pid: u32) -> String {
    // SAFETY: opened for query only, closed by `Owned`.
    let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else {
        return String::new();
    };
    let process = Owned(process);
    let mut buf = vec![0u16; 260];
    let mut len = buf.len() as u32;
    // SAFETY: the buffer and its length are handed as a pair; on success `len`
    // is the count written.
    let ok = unsafe { QueryFullProcessImageNameW(process.0, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut len) };
    if ok.is_err() {
        return String::new();
    }
    let full = String::from_utf16_lossy(&buf[..len as usize]);
    full.rsplit(['\\', '/']).next().unwrap_or(&full).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::endpoint::{Listener, Stream};
    use std::time::Duration;
    use windows::Win32::Security::{
        CreateRestrictedToken, DuplicateTokenEx, ImpersonateLoggedOnUser, SecurityImpersonation, SetTokenInformation, TokenImpersonation,
        CREATE_RESTRICTED_TOKEN_FLAGS, SID_AND_ATTRIBUTES, TOKEN_ADJUST_DEFAULT, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_IMPERSONATE,
    };

    /// Runs `open` on a thread of its own that takes on `token` first, as a
    /// process with that token would open the pipe.
    fn as_token<T: Send + 'static>(token: Owned, open: impl FnOnce() -> T + Send + 'static) -> T {
        struct Token(Owned);
        // SAFETY: a kernel handle, usable from any thread.
        unsafe impl Send for Token {}
        let token = Token(token);
        std::thread::spawn(move || {
            let token = token;
            // SAFETY: an impersonation token this test made; the thread ends
            // after `open`, and its impersonation with it.
            unsafe { ImpersonateLoggedOnUser(token.0 .0) }.expect("impersonate");
            open()
        })
        .join()
        .unwrap()
    }

    /// An impersonation token copied from this process's own.
    fn duplicate() -> Owned {
        let mut dup = HANDLE::default();
        let mut mine = HANDLE::default();
        // SAFETY: own token reopened with the rights a duplicate needs.
        unsafe {
            OpenProcessToken(GetCurrentProcess(), TOKEN_DUPLICATE | TOKEN_QUERY, &mut mine).unwrap();
            let mine = Owned(mine);
            DuplicateTokenEx(
                mine.0,
                TOKEN_QUERY | TOKEN_IMPERSONATE | TOKEN_DUPLICATE | TOKEN_ADJUST_DEFAULT | TOKEN_ASSIGN_PRIMARY,
                None,
                SecurityImpersonation,
                TokenImpersonation,
                &mut dup,
            )
            .unwrap();
        }
        Owned(dup)
    }

    /// This process's token, restricted to its own user SID: Windows checks
    /// a restricted token twice, and with the user among its restricting SIDs
    /// it passes the pipe's DACL. Only the peer check is left to refuse it.
    fn restricted_to_own_user() -> Owned {
        let dup = duplicate();
        // SAFETY: TokenUser's SID lives in `buf`, alive for the call.
        unsafe {
            let buf = info(dup.0, TokenUser).unwrap();
            let user = (*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid;
            let restrict = [SID_AND_ATTRIBUTES { Sid: user, Attributes: 0 }];
            let mut restricted = HANDLE::default();
            CreateRestrictedToken(dup.0, CREATE_RESTRICTED_TOKEN_FLAGS(0), None, None, Some(&restrict), &mut restricted).unwrap();
            Owned(restricted)
        }
    }

    /// This process's token at low integrity, as a browser's sandbox runs.
    fn low_integrity() -> Owned {
        let dup = duplicate();
        // S-1-16-4096, the low mandatory level.
        let mut low = [1u8, 1, 0, 0, 0, 0, 0, 16, 0, 0x10, 0, 0];
        let label = TOKEN_MANDATORY_LABEL {
            Label: SID_AND_ATTRIBUTES { Sid: windows::Win32::Security::PSID(low.as_mut_ptr().cast()), Attributes: 0x20 },
        };
        // SAFETY: the label and its SID outlive the call. SE_GROUP_INTEGRITY
        // (0x20) as Windows asks of an integrity label.
        unsafe {
            SetTokenInformation(
                dup.0,
                TokenIntegrityLevel,
                (&label as *const TOKEN_MANDATORY_LABEL).cast(),
                size_of::<TOKEN_MANDATORY_LABEL>() as u32,
            )
            .unwrap()
        };
        dup
    }

    fn serving() -> (Listener, String) {
        let name = crate::endpoint::fresh_name();
        (Listener::bind(&name).unwrap(), name)
    }

    /// A restricted token that the DACL lets through is still turned away,
    /// by the token itself: the check holds if a sandbox's token ever passes
    /// the DACL (Codex 0.167.0's does not).
    #[test]
    fn a_restricted_client_that_passes_the_dacl_is_refused_by_its_token() {
        let (listener, name) = serving();
        let client = std::thread::spawn({
            let name = name.clone();
            move || as_token(restricted_to_own_user(), move || Stream::connect(&name, Duration::from_secs(5)))
        });
        let server = listener.accept().expect("the restricted client gets through the DACL");
        let facts = peer_facts(&server).expect("facts");
        assert!(facts.restricted && facts.same_user, "{facts:?}");
        assert_eq!(super::super::refuse_reason(&server).as_deref().map(|r| r.ends_with("a restricted token (a CLI sandbox)")), Some(true));
        drop(client.join().unwrap());
    }

    /// The whole of `serve` holds the check: a restricted client the DACL lets
    /// through, sending a well-formed hello, hears nothing back and is let go.
    #[test]
    fn serve_refuses_a_restricted_client_before_its_hello() {
        use std::io::Read;
        let (listener, name) = serving();
        let client = std::thread::spawn(move || {
            as_token(restricted_to_own_user(), move || {
                let mut c = Stream::connect(&name, Duration::from_secs(5)).expect("through the DACL");
                let args = serde_json::json!({ "proto": crate::ipc::PROTO, "role": "ui" });
                let _ = crate::ipc::send(&mut c, crate::ipc::REQ, &crate::ipc::Req { id: 1, cmd: "hello".into(), args });
                c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut heard = Vec::new();
                let _ = (&c).read_to_end(&mut heard);
                heard
            })
        });
        let server = listener.accept().unwrap();
        assert!(crate::coreproc::serve_for_test(server).is_ok(), "refused before the hello, not ended at it");
        assert!(client.join().unwrap().is_empty(), "no welcome, no reason");
    }

    /// A client below medium integrity cannot open the pipe at all: its label
    /// refuses lower integrities reading and writing alike.
    #[test]
    fn a_low_integrity_client_cannot_open_the_pipe() {
        use windows::Win32::Foundation::GENERIC_READ;
        use windows::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAG_OVERLAPPED, FILE_SHARE_NONE, OPEN_EXISTING};
        let (_listener, name) = serving();
        let read_only = name.clone();
        let opened = as_token(low_integrity(), move || Stream::connect(&name, Duration::from_secs(2)));
        let err = opened.err().expect("refused at the open");
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied, "{err:?}");
        // Nor for reading alone, which an object's default label (no write up
        // only) would let through: the pipe's own label says no read up too.
        let opened = as_token(low_integrity(), move || {
            let wide: Vec<u16> = read_only.encode_utf16().chain(Some(0)).collect();
            // SAFETY: a NUL-terminated name; a handle got is closed at once.
            unsafe {
                CreateFileW(
                    windows::core::PCWSTR(wide.as_ptr()),
                    GENERIC_READ.0,
                    FILE_SHARE_NONE,
                    None,
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED,
                    None,
                )
            }
            .map(|h| drop(Owned(h)))
            .is_ok()
        });
        assert!(!opened, "a low-integrity read-only open got through");
    }

    /// A client that opens the pipe anonymously gives the core no token to
    /// read, and is refused for it.
    #[test]
    fn an_anonymous_client_is_refused() {
        use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
        use windows::Win32::Storage::FileSystem::{
            CreateFileW, FILE_FLAG_OVERLAPPED, FILE_SHARE_NONE, OPEN_EXISTING, SECURITY_ANONYMOUS, SECURITY_SQOS_PRESENT,
        };
        let (listener, name) = serving();
        let client = std::thread::spawn(move || {
            let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            // SAFETY: a NUL-terminated name; the handle is closed below.
            let h = unsafe {
                CreateFileW(
                    windows::core::PCWSTR(wide.as_ptr()),
                    GENERIC_READ.0 | GENERIC_WRITE.0,
                    FILE_SHARE_NONE,
                    None,
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_ANONYMOUS,
                    None,
                )
            }
            .expect("an anonymous open of the pipe");
            std::thread::sleep(Duration::from_millis(500));
            drop(Owned(h));
        });
        let server = listener.accept().unwrap();
        assert_eq!(peer_facts(&server), None);
        assert!(super::super::refuse_reason(&server).is_some());
        client.join().unwrap();
    }

    /// A client that opens the pipe without a static security context (no
    /// `SECURITY_SQOS_PRESENT`, as a shell's redirect does) cannot be
    /// impersonated before it writes (`ERROR_CANNOT_IMPERSONATE`): there is no
    /// token to read, and it is refused. Luna's own clients always pass one.
    #[test]
    fn a_client_without_a_static_security_context_is_refused() {
        use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
        use windows::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAG_OVERLAPPED, FILE_SHARE_NONE, OPEN_EXISTING};
        let (listener, name) = serving();
        let client = std::thread::spawn(move || {
            let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            // SAFETY: a NUL-terminated name; the handle is closed below.
            let h = unsafe {
                CreateFileW(
                    windows::core::PCWSTR(wide.as_ptr()),
                    GENERIC_READ.0 | GENERIC_WRITE.0,
                    FILE_SHARE_NONE,
                    None,
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED,
                    None,
                )
            }
            .expect("a plain open of the pipe");
            std::thread::sleep(Duration::from_millis(500));
            drop(Owned(h));
        });
        let server = listener.accept().unwrap();
        assert_eq!(peer_facts(&server), None);
        assert!(super::super::refuse_reason(&server).is_some());
        client.join().unwrap();
    }

    #[test]
    fn the_cores_own_facts_read() {
        let core = core_facts(false).expect("the core's own token");
        assert!(core.integrity >= super::super::MEDIUM, "{core:?}");
    }
}
