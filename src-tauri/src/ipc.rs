//! The wire between the window and the core (coreproc.rs, corelink.rs).
//!
//! Loopback TCP on a port the core picks, like the hooks' listener (hub.rs):
//! full duplex on every platform with nothing but std, where a Windows named
//! pipe needs overlapped I/O to read and write at once. Nothing on the port
//! answers without the token from `core.json`, a file in the user's own data
//! folder.
//!
//! A frame is `u32 length (LE) | u8 kind | payload`, the payload JSON. The
//! first request on every connection is `hello`; its answer says which build
//! the core is, and a window never works with a core of another build.

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::io::{self, Read, Write};
use std::path::PathBuf;

/// Bumped when a frame or the hello changes shape. The rest of the protocol
/// may change freely: a window only talks on with a core of its own build.
pub const PROTO: u32 = 1;
/// Room for a session's whole scrollback in one frame, and no more.
const MAX_FRAME: usize = 8 << 20;

/// Window → core, answered by a RES with the same id.
pub const REQ: u8 = 1;
pub const RES: u8 = 2;
/// Core → window: what the core's `emit::to_ui` sends, passed on to the page.
pub const EVENT: u8 = 3;
/// Window → core, never answered: keystrokes and resizes, which must reach
/// the session in the order they were made and are worth nothing late.
pub const SEND: u8 = 5;

#[derive(Serialize, Deserialize)]
pub struct Req {
    pub id: u64,
    pub cmd: String,
    #[serde(default)]
    pub args: serde_json::Value,
}

#[derive(Serialize, Deserialize)]
pub struct Res {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub err: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct Event {
    pub event: String,
    pub payload: serde_json::Value,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    pub proto: u32,
    pub token: String,
    /// `ui` for the window that stays connected, `ctl` for a one-off command.
    pub role: String,
}

/// Who the core is: the answer to `hello` and to `info`.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Welcome {
    pub pid: u32,
    pub version: String,
    pub build: String,
    pub fingerprint: String,
    /// Epoch ms.
    pub started: u64,
    /// Sessions running in it, background helpers in a turn among them: a
    /// core of another build with none is replaced without a word, one with
    /// some only when the user says so. Absent from a core older than
    /// sessions in the core, which has none.
    #[serde(default)]
    pub sessions: usize,
}

impl Welcome {
    pub fn of_this_process(started: u64) -> Welcome {
        Welcome {
            pid: std::process::id(),
            version: env!("CARGO_PKG_VERSION").into(),
            build: env!("LUNA_BUILD").into(),
            fingerprint: env!("LUNA_CORE_FP").into(),
            started,
            sessions: crate::pty::manager().live_count() + crate::background::running(),
        }
    }
}

/// Where to find the running core, written by it once it listens.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Discovery {
    pub pid: u32,
    pub port: u16,
    pub token: String,
    /// The core process's creation time, epoch ms (0 from a core that did not
    /// write it): after a reboot the pid is often some other program's, and
    /// only this tells the two apart without a connect that has to time out.
    #[serde(default)]
    pub created_ms: u64,
}

impl Discovery {
    /// Whether the core this names is still the process running under its pid.
    pub fn running(&self) -> bool {
        crate::procs::alive(self.pid)
            && (self.created_ms == 0 || crate::procs::created_ms(self.pid).is_none_or(|c| c == self.created_ms))
    }
}

/// `<data>/core/`: the discovery file, the lock that keeps the core single,
/// and on Windows the copies of the exe the core runs from.
pub fn core_dir() -> PathBuf {
    crate::paths::data_dir().join("core")
}

pub fn discovery_file() -> PathBuf {
    core_dir().join("core.json")
}

pub fn read_frame(r: &mut impl Read) -> io::Result<(u8, Vec<u8>)> {
    let mut head = [0u8; 5];
    r.read_exact(&mut head)?;
    let len = u32::from_le_bytes([head[0], head[1], head[2], head[3]]) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
    }
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload)?;
    Ok((head[4], payload))
}

pub fn send<T: Serialize>(w: &mut impl Write, kind: u8, msg: &T) -> io::Result<()> {
    w.write_all(&encode(kind, msg)?)?;
    w.flush()
}

/// A whole frame as bytes, for a writer that queues frames rather than
/// writing them where they are made.
pub fn encode<T: Serialize>(kind: u8, msg: &T) -> io::Result<Vec<u8>> {
    let payload = serde_json::to_vec(msg).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    if payload.len() > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "frame too large"));
    }
    let mut buf = Vec::with_capacity(5 + payload.len());
    buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    buf.push(kind);
    buf.extend_from_slice(&payload);
    Ok(buf)
}

pub fn recv<T: DeserializeOwned>(r: &mut impl Read, want: u8) -> io::Result<T> {
    let (kind, payload) = read_frame(r)?;
    if kind != want {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("frame of kind {kind}, wanted {want}")));
    }
    serde_json::from_slice(&payload).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// A token nobody can guess: hashed from the OS-seeded keys std hands every
/// `RandomState`, with the time and the pid for good measure.
pub fn mint_token() -> String {
    use sha2::{Digest, Sha256};
    use std::hash::{BuildHasher, Hasher};
    let mut h = Sha256::new();
    for i in 0..4u64 {
        let mut k = std::collections::hash_map::RandomState::new().build_hasher();
        k.write_u64(i);
        h.update(k.finish().to_le_bytes());
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    h.update(nanos.to_le_bytes());
    h.update(std::process::id().to_le_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_and_refuse_the_oversized() {
        let mut buf = Vec::new();
        send(&mut buf, REQ, &Req { id: 7, cmd: "info".into(), args: serde_json::json!({ "a": 1 }) }).unwrap();
        send(&mut buf, RES, &serde_json::json!({})).unwrap();
        let mut r = io::Cursor::new(buf);
        let req: Req = recv(&mut r, REQ).unwrap();
        assert_eq!((req.id, req.cmd.as_str(), req.args["a"].as_i64()), (7, "info", Some(1)));
        assert!(recv::<Req>(&mut r, REQ).is_err(), "a response is not a request");

        let mut big = io::Cursor::new([((MAX_FRAME + 1) as u32).to_le_bytes().to_vec(), vec![REQ]].concat());
        assert!(read_frame(&mut big).is_err());
    }

    #[test]
    fn tokens_differ() {
        assert_ne!(mint_token(), mint_token());
        assert_eq!(mint_token().len(), 64);
    }
}
