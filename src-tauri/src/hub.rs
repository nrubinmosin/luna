//! The door the CLIs knock on: a loopback HTTP listener the sessions' hooks
//! post to. It is how Luna learns the exact moment a Claude Code turn starts,
//! ends, or stops for a permission — without polling the registry and without
//! a word reaching the model (the hook commands print nothing, and nothing
//! silent costs context).
//!
//! Loopback only, on a port picked at start; every request carries a secret
//! minted for this run, so a stray local process cannot feed the activity
//! tracker. Each session's hook file names the port and the secret, and is
//! rewritten at spawn, so a session that outlives a Luna restart simply stops
//! being heard and the registry poll covers it.

use std::io::Read;
use std::sync::OnceLock;

struct Listener {
    port: u16,
    secret: String,
}

static LISTENER: OnceLock<Listener> = OnceLock::new();

/// Starts the listener on its own thread. Called once from setup; a failure
/// to bind is logged and the app runs on the registry poll alone.
pub fn start() {
    let server = match tiny_http::Server::http("127.0.0.1:0") {
        Ok(s) => s,
        Err(e) => {
            crate::log::error("hub", &format!("cannot listen on loopback, hooks are off: {e}"));
            return;
        }
    };
    let Some(addr) = server.server_addr().to_ip() else {
        crate::log::error("hub", "listener has no ip address; hooks are off");
        return;
    };
    let secret = mint_secret();
    let _ = LISTENER.set(Listener { port: addr.port(), secret });
    crate::log::info("hub", &format!("listening on {addr}"));

    std::thread::Builder::new()
        .name("luna-hub".into())
        .spawn(move || {
            // A thread per request: an MCP `wait` holds its request for
            // minutes, and a hook must not queue behind it.
            for mut req in server.incoming_requests() {
                std::thread::spawn(move || {
                    let path = req.url().to_string();
                    if path == "/mcp" || path.starts_with("/mcp?") {
                        let (status, body) = handle_mcp(&mut req);
                        let _ = match body {
                            Some(json) => req.respond(
                                tiny_http::Response::from_string(json)
                                    .with_status_code(status)
                                    .with_header(tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap()),
                            ),
                            None => req.respond(tiny_http::Response::empty(status)),
                        };
                    } else {
                        let status = handle(&mut req);
                        let _ = req.respond(tiny_http::Response::empty(status));
                    }
                });
            }
        })
        .expect("spawn hub thread");
}

/// The URL a session's hooks post to, or None while the listener is off.
pub fn hook_url(chat_id: &str) -> Option<String> {
    let l = LISTENER.get()?;
    Some(format!("http://127.0.0.1:{}/hook/{}/{}", l.port, chat_id, l.secret))
}

/// The MCP endpoint, or None while the listener is off.
pub fn mcp_url() -> Option<String> {
    let l = LISTENER.get()?;
    Some(format!("http://127.0.0.1:{}/mcp", l.port))
}

/// `POST /mcp` with `Authorization: Bearer <session token>`. GET is refused
/// (no server-initiated stream is offered), which the transport allows.
fn handle_mcp(req: &mut tiny_http::Request) -> (u16, Option<String>) {
    if *req.method() != tiny_http::Method::Post {
        return (405, None);
    }
    let token = req
        .headers()
        .iter()
        .find(|h| h.field.equiv("Authorization"))
        .map(|h| h.value.as_str().to_string())
        .and_then(|v| v.strip_prefix("Bearer ").map(str::to_owned))
        .unwrap_or_default();
    let Some(caller) = crate::agents::caller_of(token.trim()) else {
        return (401, Some(r#"{"error":"unknown or expired session token"}"#.into()));
    };
    let mut body = String::new();
    let _ = req.as_reader().take(4 * 1024 * 1024).read_to_string(&mut body);
    match crate::mcp::handle(&caller, &body) {
        Some(v) => (200, Some(v.to_string())),
        None => (202, None),
    }
}

fn handle(req: &mut tiny_http::Request) -> u16 {
    if *req.method() != tiny_http::Method::Post {
        return 405;
    }
    let path = req.url().to_string();
    let Some((chat_id, secret)) = parse_hook_path(&path) else { return 404 };
    let Some(l) = LISTENER.get() else { return 503 };
    if secret != l.secret {
        return 403;
    }
    // The hook's stdin, forwarded as the body: a small JSON object.
    let mut body = String::new();
    let _ = req.as_reader().take(64 * 1024).read_to_string(&mut body);
    match crate::activity::HookEvent::parse(&body) {
        Some(ev) => {
            crate::activity::on_hook(chat_id, ev);
            204
        }
        None => 400,
    }
}

/// `/hook/<chat id>/<secret>` → (chat id, secret).
fn parse_hook_path(path: &str) -> Option<(&str, &str)> {
    let rest = path.strip_prefix("/hook/")?;
    let rest = rest.split('?').next()?;
    let (id, secret) = rest.split_once('/')?;
    (!id.is_empty() && !secret.is_empty() && !secret.contains('/')).then_some((id, secret))
}

/// Unguessable enough for loopback: this process, this instant, and the
/// address of a stack slot, hashed.
fn mint_secret() -> String {
    use sha2::{Digest, Sha256};
    let slot = 0u8;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut h = Sha256::new();
    h.update(std::process::id().to_le_bytes());
    h.update(nanos.to_le_bytes());
    h.update((&slot as *const u8 as usize).to_le_bytes());
    let out = h.finalize();
    out.iter().take(16).map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hook_paths() {
        assert_eq!(parse_hook_path("/hook/abc/s3cr3t"), Some(("abc", "s3cr3t")));
        assert_eq!(parse_hook_path("/hook/abc/s3cr3t?x=1"), Some(("abc", "s3cr3t")));
        assert_eq!(parse_hook_path("/hook/abc"), None);
        assert_eq!(parse_hook_path("/hook//s"), None);
        assert_eq!(parse_hook_path("/other/abc/s"), None);
    }
}

#[cfg(test)]
mod live {
    use super::*;

    /// The real listener, a real POST the way curl would make it, and the
    /// activity tracker on the other side.
    #[test]
    fn a_posted_hook_reaches_the_tracker() {
        start();
        let url = hook_url("chat-hub-test").expect("listener up");
        let post = |body: &str| ureq::post(&url).send_string(body).map(|r| r.status());

        assert_eq!(post(r#"{"hook_event_name":"UserPromptSubmit","session_id":"s"}"#).unwrap(), 204);
        assert_eq!(crate::activity::hook_turn("chat-hub-test"), Some(crate::activity::Turn::Busy));
        assert_eq!(post(r#"{"hook_event_name":"Notification","notification_type":"permission_prompt"}"#).unwrap(), 204);
        assert_eq!(crate::activity::hook_turn("chat-hub-test"), Some(crate::activity::Turn::Waiting));
        assert_eq!(post(r#"{"hook_event_name":"Stop"}"#).unwrap(), 204);
        assert_eq!(crate::activity::hook_turn("chat-hub-test"), Some(crate::activity::Turn::Idle));

        // A bad secret, a bad body and a bad method are told apart from a good one.
        let (base, _secret) = url.rsplit_once('/').unwrap();
        let bad = format!("{base}/nope");
        assert_eq!(ureq::post(&bad).send_string("{}").unwrap_err().into_response().map(|r| r.status()), Some(403));
        assert_eq!(post("not json").unwrap_err().into_response().map(|r| r.status()), Some(400));
        assert_eq!(ureq::get(&url).call().unwrap_err().into_response().map(|r| r.status()), Some(405));
    }
}
