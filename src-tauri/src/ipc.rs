//! The wire between the window and the core (coreproc.rs, corelink.rs).
//!
//! The core's own endpoint (endpoint/): a named pipe on Windows, a Unix socket
//! elsewhere, that only this user's unconfined processes are let into
//! (peer.rs). There is no token: what the OS says of the process that
//! connected is the key, and nothing on disk is.
//!
//! A frame is `u32 length (LE) | u8 kind | payload`, the payload JSON. The
//! first request on every connection is `hello`; its answer says which build
//! the core is, and a window never works with a core of another build.

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::io::{self, Read, Write};
use std::path::PathBuf;

/// Bumped when a frame, the hello or the discovery file changes shape. The
/// rest of the protocol may change freely: a window only talks on with a core
/// of its own build. 2: the endpoint by OS identity, no token anywhere; 1 was
/// loopback TCP and a token in `core.json` (corelink/v1.rs).
pub const PROTO: u32 = 2;
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
    /// Why this core starts no session or helper: it runs as administrator
    /// (elevation.rs). Absent from a core that does, and from one older than
    /// the check, whose elevation the window reads from its token instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
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
            refused: crate::elevation::refusal().map(String::from),
        }
    }
}

/// Where to find the running core, written by it once it listens: which
/// process it is and the name of its endpoint. Nothing in it lets anyone in;
/// a window checks that the endpoint's server is this pid before it speaks.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Discovery {
    pub pid: u32,
    /// The core process's creation time, epoch ms: after a reboot the pid is
    /// often some other program's, and only this tells the two apart without
    /// a connect that has to time out.
    pub created_ms: u64,
    pub proto: u32,
    /// The endpoint's name (endpoint.rs `fresh_name`).
    pub endpoint: String,
}

/// What a core of protocol 1 (Luna 0.11 and before) wrote: a loopback port
/// and the token every command needed. Read, never written: a window uses it
/// only to ask that core how many sessions it runs and to stop it
/// (corelink/v1.rs).
#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LegacyDiscovery {
    pub pid: u32,
    pub port: u16,
    pub token: String,
    #[serde(default)]
    pub created_ms: u64,
}

/// The core a `core.json` names, by the protocol it speaks.
#[derive(Clone)]
pub enum Found {
    Current(Discovery),
    V1(LegacyDiscovery),
}

impl Found {
    /// A discovery file's contents: one with a token is a protocol-1 core's.
    pub fn parse(bytes: &[u8]) -> Option<Found> {
        let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
        if value.get("token").is_some() {
            return serde_json::from_value(value).ok().map(Found::V1);
        }
        serde_json::from_value(value).ok().map(Found::Current)
    }

    pub fn pid(&self) -> u32 {
        match self {
            Found::Current(d) => d.pid,
            Found::V1(d) => d.pid,
        }
    }

    fn created_ms(&self) -> u64 {
        match self {
            Found::Current(d) => d.created_ms,
            Found::V1(d) => d.created_ms,
        }
    }

    /// Whether the core this names is still the process running under its pid.
    pub fn running(&self) -> bool {
        let (pid, created) = (self.pid(), self.created_ms());
        crate::procs::alive(pid) && (created == 0 || crate::procs::created_ms(pid).is_none_or(|c| c == created))
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
    read_frame_within(r, MAX_FRAME)
}

/// A frame of at most `max` bytes, refused before its payload is read.
pub fn read_frame_within(r: &mut impl Read, max: usize) -> io::Result<(u8, Vec<u8>)> {
    let mut head = [0u8; 5];
    r.read_exact(&mut head)?;
    let len = u32::from_le_bytes([head[0], head[1], head[2], head[3]]) as usize;
    if len > max {
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

    /// What a core writes for anyone of this user's to read holds nothing
    /// that lets them in: no token, no port, nothing but which process it is
    /// and where it listens. A field added here is a decision, not a detail.
    #[test]
    fn discovery_carries_no_credential() {
        let d = Discovery { pid: 42, created_ms: 7, proto: PROTO, endpoint: crate::endpoint::fresh_name() };
        let written: serde_json::Value = serde_json::to_value(&d).unwrap();
        let keys: Vec<&str> = written.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["pid", "createdMs", "proto", "endpoint"]);
        let text = serde_json::to_string(&d).unwrap();
        assert!(!text.contains("token") && !text.contains("port"), "{text}");
        // And it reads back as what it is.
        assert!(matches!(Found::parse(text.as_bytes()), Some(Found::Current(back)) if back == d));
    }

    /// A protocol-1 core's file, as Luna 0.11.0 wrote it, reads as one; a
    /// file of neither shape reads as nothing.
    #[test]
    fn a_protocol_1_core_is_told_apart_by_its_token() {
        let v1 = br#"{"pid":19344,"port":49846,"token":"00ff","createdMs":1794198535413}"#;
        match Found::parse(v1) {
            Some(Found::V1(d)) => assert_eq!((d.pid, d.port, d.token.as_str(), d.created_ms), (19344, 49846, "00ff", 1794198535413)),
            _ => panic!("not read as a protocol-1 core"),
        }
        assert!(Found::parse(br#"{"pid":1}"#).is_none());
        assert!(Found::parse(br#"{"pid":1,"port":2,"createdMs":3}"#).is_none());
        assert!(Found::parse(b"not json").is_none());
    }
}
