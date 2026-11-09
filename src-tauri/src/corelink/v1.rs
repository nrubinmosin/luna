//! A core of protocol 1 (Luna 0.11 and before), reached the way it listens:
//! loopback TCP, with the token from its `core.json`.
//!
//! Such a core is always another build's, so a window never works with it.
//! It is asked who it is, for the number of its sessions, and, where it is to
//! be replaced, to stop; nothing else is ever sent. `Link` has no other way
//! to say anything, and is dropped once the core is gone or left alone.

use crate::ipc::{self, LegacyDiscovery, Req, Res, Welcome};
use serde::Serialize;
use std::io;
use std::net::TcpStream;
use std::time::Duration;

/// The protocol-1 hello: the token was its key.
#[derive(Serialize)]
struct Hello<'a> {
    proto: u32,
    token: &'a str,
    role: &'a str,
}

/// A protocol-1 core said hello to.
pub(super) struct Link(TcpStream);

/// Says hello to the protocol-1 core `found` names, as `role`.
pub(super) fn hello(found: &LegacyDiscovery, role: &str) -> io::Result<(Link, Welcome)> {
    let mut stream = TcpStream::connect_timeout(&([127, 0, 0, 1], found.port).into(), Duration::from_secs(2))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let args = serde_json::to_value(Hello { proto: 1, token: &found.token, role })?;
    ipc::send(&mut stream, ipc::REQ, &Req { id: 1, cmd: "hello".into(), args })?;
    let res: Res = ipc::recv(&mut stream, ipc::RES)?;
    match (res.ok, res.err) {
        (Some(ok), None) => Ok((Link(stream), serde_json::from_value(ok)?)),
        (_, err) => Err(io::Error::other(err.unwrap_or_else(|| "refused".into()))),
    }
}

impl Link {
    /// Asks the core to stop and waits for it to be gone (corelink.rs `stop`).
    pub(super) fn stop(self, pid: u32) {
        super::stop(self.0, pid);
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    /// A stand-in for a Luna 0.11 core: answers a hello with the token with
    /// `sessions` running, a shutdown by closing, and records every command
    /// it hears and the hello it was sent.
    pub(in crate::corelink) struct OldCore {
        pub found: LegacyDiscovery,
        pub heard: Arc<Mutex<Vec<String>>>,
        pub hello: Arc<Mutex<Option<serde_json::Value>>>,
    }

    pub(in crate::corelink) fn old_core(sessions: usize) -> OldCore {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let token = crate::secret::random_hex(32);
        let heard = Arc::new(Mutex::new(Vec::new()));
        let hello = Arc::new(Mutex::new(None));
        let (h, said, expected) = (Arc::clone(&heard), Arc::clone(&hello), token.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut s = stream;
                while let Ok(req) = ipc::recv::<Req>(&mut s, ipc::REQ) {
                    h.lock().unwrap().push(req.cmd.clone());
                    let res = match req.cmd.as_str() {
                        "hello" if req.args["token"] == expected && req.args["proto"] == 1 => {
                            *said.lock().unwrap() = Some(req.args.clone());
                            let core = Welcome {
                                pid: 1,
                                version: "0.11.0".into(),
                                build: "f6d9636".into(),
                                fingerprint: "1e658c94aebe9013".into(),
                                started: 0,
                                sessions,
                                refused: None,
                            };
                            Res { id: req.id, ok: serde_json::to_value(core).ok(), err: None }
                        }
                        "shutdown" => {
                            let _ = ipc::send(&mut s, ipc::RES, &Res { id: req.id, ok: Some(serde_json::Value::Null), err: None });
                            break;
                        }
                        _ => Res { id: req.id, ok: None, err: Some("no".into()) },
                    };
                    if ipc::send(&mut s, ipc::RES, &res).is_err() {
                        break;
                    }
                }
            }
        });
        let found = LegacyDiscovery { pid: u32::MAX, port, token, created_ms: 0 };
        OldCore { found, heard, hello }
    }
}
