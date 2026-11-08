//! Who is at the other end of a loopback connection to the core, on Windows.
//!
//! The core's token in `core/core.json` is the key to every command, and a
//! Codex session in its Windows sandbox reads that file (the sandbox's
//! restricted token reads what the user can, the folder's protected DACL
//! included) and connects to loopback (threat model F3). The read deny Luna
//! gives a Codex session off Windows cannot be given there: the unelevated
//! sandbox refuses every command under a deny. So the core turns such a
//! connection away itself, before the hello: it finds the process that owns
//! the connection's client end and refuses it when its token is restricted,
//! an AppContainer, or of a lower integrity than the core's — and when the
//! owner cannot be found at all (fail closed). A window and the test build run
//! with a plain token of the core's own integrity and are let through.
//!
//! The decision (`verdict`) is pure and knows nothing of Windows, so a test
//! feeds it the facts a connection would carry; the facts themselves
//! (`peer_facts`) are Windows' to read.

use std::net::TcpStream;

/// What the peer check reads of the process at the client end of a loopback
/// connection. `integrity` is its token's mandatory-label RID
/// (`SECURITY_MANDATORY_*_RID`): medium is `0x2000`. Off Windows only the
/// tests build it.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Clone, Debug, PartialEq)]
pub struct PeerFacts {
    pub pid: u32,
    pub image: String,
    pub restricted: bool,
    pub app_container: bool,
    pub integrity: u32,
}

/// Whether a connection is let through, and why not when it is turned away.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Verdict {
    Allow,
    /// Let in to stop the core and nothing else: a plain process below the
    /// integrity of a core run as administrator, which runs no session and
    /// so has nothing else to give (elevation.rs). The window it started
    /// from, not elevated, has no other way to stop it.
    StopOnly,
    /// Refused, with a reason safe to log (no token, no secret).
    Refuse(&'static str),
}

/// The decision, fail closed: a peer whose owner could not be found, whose
/// token is restricted or an AppContainer, or whose integrity is below the
/// core's, is refused; anyone else at the core's integrity or above is let in
/// to try the hello. `core_integrity` is the core's own mandatory-label RID.
#[cfg_attr(not(test), allow(dead_code))]
pub fn verdict(peer: Option<&PeerFacts>, core_integrity: u32) -> Verdict {
    verdict_for(peer, core_integrity, false)
}

/// `verdict`, for a core that refuses all work when `core_refuses_work`: then
/// a plain peer of lower integrity may stop it (`StopOnly`). A sandboxed,
/// contained or unknown peer is refused all the same.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn verdict_for(peer: Option<&PeerFacts>, core_integrity: u32, core_refuses_work: bool) -> Verdict {
    match peer {
        None => Verdict::Refuse("the owning process could not be found"),
        Some(p) if p.restricted => Verdict::Refuse("a restricted token (a CLI sandbox)"),
        Some(p) if p.app_container => Verdict::Refuse("an app container"),
        Some(p) if p.integrity < core_integrity && core_refuses_work => Verdict::StopOnly,
        Some(p) if p.integrity < core_integrity => Verdict::Refuse("a lower integrity level"),
        Some(_) => Verdict::Allow,
    }
}

/// How far `stream`'s client is let in: `Ok(false)` all the way to the token
/// check, `Ok(true)` to stop the core only, or why it is refused. On Windows
/// it reads the owning process and decides; elsewhere, where Luna never
/// confines a CLI in an OS sandbox, every loopback peer is let in to the
/// token check that follows. The reason names the process, never the token,
/// for the log.
#[cfg(windows)]
pub fn admission(stream: &TcpStream, core_refuses_work: bool) -> Result<bool, String> {
    // Without the core's own integrity there is no threshold to judge a peer
    // against, so no connection is let in until it can be read (fail closed).
    let Some(core) = win::core_integrity() else {
        return Err("the core's own integrity could not be read".to_string());
    };
    let facts = win::peer_facts(stream);
    match verdict_for(facts.as_ref(), core, core_refuses_work) {
        Verdict::Allow => Ok(false),
        Verdict::StopOnly => Ok(true),
        Verdict::Refuse(why) => Err(match &facts {
            Some(f) => format!("pid {} ({}): {why}", f.pid, f.image),
            None => why.to_string(),
        }),
    }
}

/// Off Windows there is no OS sandbox under Luna to escape: the token check is
/// the gate.
#[cfg(not(windows))]
pub fn admission(_stream: &TcpStream, _core_refuses_work: bool) -> Result<bool, String> {
    Ok(false)
}

/// The reason to refuse `stream`'s client from a core that works, or None to
/// let it through (`admission`).
#[cfg_attr(not(all(test, windows)), allow(dead_code))]
pub fn refuse_reason(stream: &TcpStream) -> Option<String> {
    admission(stream, false).err()
}

#[cfg(windows)]
mod win {
    use super::PeerFacts;
    use std::net::{SocketAddr, TcpStream};
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCP6TABLE_OWNER_PID, MIB_TCPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_CONNECTIONS,
    };
    use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6};
    use windows::Win32::Security::{GetSidSubAuthority, GetSidSubAuthorityCount, IsTokenRestricted};
    use windows::Win32::Security::{GetTokenInformation, TokenIntegrityLevel, TokenIsAppContainer, TOKEN_MANDATORY_LABEL, TOKEN_QUERY};
    use windows::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    /// The core's own mandatory-label RID: what a peer must meet or beat,
    /// Medium (`0x2000`) in the ordinary run. None when it cannot be read, on
    /// which the caller refuses rather than guess a threshold. Not cached: a
    /// stale None would refuse every connection for the core's life, and the
    /// read is a couple of local calls.
    pub(super) fn core_integrity() -> Option<u32> {
        // SAFETY: the current-process token is opened for query and closed
        // here; the pseudo-handle needs no close.
        unsafe {
            let mut token = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).ok()?;
            let il = token_integrity(token);
            let _ = CloseHandle(token);
            il
        }
    }

    /// The facts about the process that owns `stream`'s client end, or None
    /// when its pid cannot be found or opened — on which `verdict` fails
    /// closed. Reads the live TCP table for the pid, then the process token;
    /// holds no lock of Luna's, and each call returns at once.
    pub(super) fn peer_facts(stream: &TcpStream) -> Option<PeerFacts> {
        // The client's socket has local = the core's peer, remote = the core's
        // local end; the table is from the owner's side.
        let client_local = stream.peer_addr().ok()?;
        let client_remote = stream.local_addr().ok()?;
        let pid = owner_pid(client_local, client_remote)?;
        read_token(pid)
    }

    /// The pid owning the TCP connection whose local endpoint is `local` and
    /// remote is `remote`, from the `OWNER_PID` table of the right family.
    fn owner_pid(local: SocketAddr, remote: SocketAddr) -> Option<u32> {
        match (local, remote) {
            (SocketAddr::V4(_), SocketAddr::V4(_)) => owner_pid_v4(local, remote),
            (SocketAddr::V6(_), SocketAddr::V6(_)) => owner_pid_v6(local, remote),
            _ => None,
        }
    }

    /// The `OWNER_PID` TCP table for `family`, in a zero-filled `u64` buffer so
    /// the MIB structs read out of it are aligned. Grown until it fits (a few
    /// tries: the table can change between the size call and the read). None on
    /// any error.
    fn table_words(family: u32) -> Option<Vec<u64>> {
        let mut size = 0u32;
        for _ in 0..8 {
            // SAFETY: first with a null buffer to learn the size, then with a
            // buffer at least that large; the class and family are valid.
            let rc = unsafe {
                if size == 0 {
                    GetExtendedTcpTable(None, &mut size, false, family, TCP_TABLE_OWNER_PID_CONNECTIONS, 0)
                } else {
                    let mut buf = vec![0u64; (size as usize).div_ceil(8)];
                    let rc =
                        GetExtendedTcpTable(Some(buf.as_mut_ptr().cast()), &mut size, false, family, TCP_TABLE_OWNER_PID_CONNECTIONS, 0);
                    if rc == 0 {
                        return Some(buf);
                    }
                    rc
                }
            };
            // ERROR_INSUFFICIENT_BUFFER (122): `size` now holds what is needed.
            if rc != 0 && rc != 122 {
                return None;
            }
        }
        None
    }

    fn owner_pid_v4(local: SocketAddr, remote: SocketAddr) -> Option<u32> {
        let words = table_words(AF_INET.0 as u32)?;
        // SAFETY: the buffer begins with a MIB_TCPTABLE_OWNER_PID: a count,
        // then that many rows, all within the bytes the call reported.
        let table = unsafe { &*words.as_ptr().cast::<MIB_TCPTABLE_OWNER_PID>() };
        let rows = unsafe { std::slice::from_raw_parts(table.table.as_ptr(), table.dwNumEntries as usize) };
        for r in rows {
            let l = v4(r.dwLocalAddr, r.dwLocalPort);
            let rem = v4(r.dwRemoteAddr, r.dwRemotePort);
            if l == local && rem == remote {
                return Some(r.dwOwningPid);
            }
        }
        None
    }

    fn owner_pid_v6(local: SocketAddr, remote: SocketAddr) -> Option<u32> {
        let words = table_words(AF_INET6.0 as u32)?;
        // SAFETY: as in owner_pid_v4, for the v6 table.
        let table = unsafe { &*words.as_ptr().cast::<MIB_TCP6TABLE_OWNER_PID>() };
        let rows = unsafe { std::slice::from_raw_parts(table.table.as_ptr(), table.dwNumEntries as usize) };
        for r in rows {
            let l = v6(r.ucLocalAddr, r.dwLocalScopeId, r.dwLocalPort);
            let rem = v6(r.ucRemoteAddr, r.dwRemoteScopeId, r.dwRemotePort);
            if same_v6(&l, &local) && same_v6(&rem, &remote) {
                return Some(r.dwOwningPid);
            }
        }
        None
    }

    /// A `dwLocalAddr`/`dwLocalPort` pair as a SocketAddr: the address is a
    /// network-order `in_addr`, the port is network order in the low 16 bits.
    fn v4(addr: u32, port: u32) -> SocketAddr {
        let octets = addr.to_ne_bytes();
        SocketAddr::from((std::net::Ipv4Addr::from(octets), be_port(port)))
    }

    fn v6(addr: [u8; 16], scope: u32, port: u32) -> SocketAddr {
        SocketAddr::V6(std::net::SocketAddrV6::new(std::net::Ipv6Addr::from(addr), be_port(port), 0, scope))
    }

    /// Two v6 endpoints the same but for a scope id and flow the connection may
    /// not carry the same way: address and port decide.
    fn same_v6(a: &SocketAddr, b: &SocketAddr) -> bool {
        a.ip() == b.ip() && a.port() == b.port()
    }

    /// The port from a `dw*Port` field: its two network-order bytes.
    fn be_port(port: u32) -> u16 {
        u16::from_be_bytes([(port & 0xff) as u8, ((port >> 8) & 0xff) as u8])
    }

    /// The token facts of `pid`, or None when it cannot be opened (gone, or
    /// above this process's reach) or a fact cannot be read — `verdict` fails
    /// closed on None, so an unknown fact refuses the peer rather than pass it.
    fn read_token(pid: u32) -> Option<PeerFacts> {
        // SAFETY: the process is opened for query only and closed before
        // return; the token likewise; each Win32 call is checked.
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let image = image_name(process);
            let mut token = HANDLE::default();
            let got = OpenProcessToken(process, TOKEN_QUERY, &mut token);
            if got.is_err() {
                let _ = CloseHandle(process);
                return None;
            }
            // Each fact is required: an AppContainer or an integrity query that
            // fails leaves the peer uncharacterized, and an uncharacterized
            // peer is refused. `IsTokenRestricted` reports false for a plain
            // token and cannot say "unknown" apart from it; it does not fail
            // for a token just opened.
            let restricted = IsTokenRestricted(token).is_ok();
            let facts = token_is_app_container(token).zip(token_integrity(token));
            let _ = CloseHandle(token);
            let _ = CloseHandle(process);
            let (app_container, integrity) = facts?;
            Some(PeerFacts { pid, image, restricted, app_container, integrity })
        }
    }

    /// `pid`'s image file name (its last path component), empty when it cannot
    /// be read.
    unsafe fn image_name(process: HANDLE) -> String {
        let mut buf = vec![0u16; 260];
        let mut len = buf.len() as u32;
        // SAFETY: the buffer and its length are handed as a pair; on success
        // `len` is the count written, not counting the terminator.
        let ok = unsafe { QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut len) };
        if ok.is_err() {
            return String::new();
        }
        let full = String::from_utf16_lossy(&buf[..len as usize]);
        full.rsplit(['\\', '/']).next().unwrap_or(&full).to_string()
    }

    /// Whether `token` is an AppContainer (`TokenIsAppContainer` nonzero).
    /// None when the query fails, so the peer is refused rather than taken for
    /// not contained.
    unsafe fn token_is_app_container(token: HANDLE) -> Option<bool> {
        let mut is = 0u32;
        let mut len = 0u32;
        // SAFETY: a u32 out-parameter sized to match.
        unsafe {
            GetTokenInformation(token, TokenIsAppContainer, Some((&mut is as *mut u32).cast()), size_of::<u32>() as u32, &mut len).ok()?;
        }
        Some(is != 0)
    }

    /// The mandatory-label RID of an open `token`: the last sub-authority of
    /// its integrity SID. None on any failure.
    unsafe fn token_integrity(token: HANDLE) -> Option<u32> {
        let mut len = 0u32;
        // SAFETY: first for the size, then into a buffer of that size holding
        // a TOKEN_MANDATORY_LABEL, whose SID is read while the buffer lives.
        unsafe {
            let _ = GetTokenInformation(token, TokenIntegrityLevel, None, 0, &mut len);
            if len == 0 {
                return None;
            }
            let mut buf = vec![0u8; len as usize];
            GetTokenInformation(token, TokenIntegrityLevel, Some(buf.as_mut_ptr().cast()), len, &mut len).ok()?;
            let label = &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL);
            let sid = label.Label.Sid;
            let count = *GetSidSubAuthorityCount(sid);
            (count > 0).then(|| *GetSidSubAuthority(sid, (count - 1) as u32))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MEDIUM: u32 = 0x2000;
    const LOW: u32 = 0x1000;

    fn plain() -> PeerFacts {
        PeerFacts { pid: 100, image: "luna.exe".into(), restricted: false, app_container: false, integrity: MEDIUM }
    }

    #[test]
    fn a_plain_peer_at_the_cores_integrity_is_let_in() {
        assert_eq!(verdict(Some(&plain()), MEDIUM), Verdict::Allow);
        // Above the core's integrity is fine too.
        let high = PeerFacts { integrity: 0x3000, ..plain() };
        assert_eq!(verdict(Some(&high), MEDIUM), Verdict::Allow);
    }

    #[test]
    fn a_sandboxed_or_unknown_peer_is_refused() {
        let restricted = PeerFacts { restricted: true, ..plain() };
        let contained = PeerFacts { app_container: true, ..plain() };
        let low = PeerFacts { integrity: LOW, ..plain() };
        for (facts, which) in [(&restricted, "restricted"), (&contained, "app container"), (&low, "low integrity")] {
            assert!(matches!(verdict(Some(facts), MEDIUM), Verdict::Refuse(_)), "{which} should be refused");
        }
        // The owner could not be found: fail closed.
        assert!(matches!(verdict(None, MEDIUM), Verdict::Refuse(_)), "unknown owner is refused");
        // A restricted token of the core's integrity is still refused: the
        // sandbox keeps the integrity but restricts the token.
        assert!(matches!(verdict(Some(&restricted), MEDIUM), Verdict::Refuse(_)));
    }

    #[test]
    fn a_core_run_as_administrator_may_be_stopped_by_an_ordinary_window_and_no_sandbox() {
        const HIGH: u32 = 0x3000;
        // The ordinary window below an elevated core that refuses all work:
        // let in to stop it, nothing more.
        assert_eq!(verdict_for(Some(&plain()), HIGH, true), Verdict::StopOnly);
        // Not by a core that works: there it is refused as ever.
        assert!(matches!(verdict_for(Some(&plain()), HIGH, false), Verdict::Refuse(_)));
        // A sandboxed, contained or unknown peer is refused even by a core that
        // refuses work, at any integrity.
        for p in [PeerFacts { restricted: true, ..plain() }, PeerFacts { app_container: true, ..plain() }] {
            assert!(matches!(verdict_for(Some(&p), HIGH, true), Verdict::Refuse(_)), "{p:?}");
            let low = PeerFacts { integrity: LOW, ..p };
            assert!(matches!(verdict_for(Some(&low), HIGH, true), Verdict::Refuse(_)), "{low:?}");
        }
        assert!(matches!(verdict_for(None, HIGH, true), Verdict::Refuse(_)));
        // One of the core's own integrity has the full way in.
        assert_eq!(verdict_for(Some(&PeerFacts { integrity: HIGH, ..plain() }), HIGH, true), Verdict::Allow);
    }
}
