//! The door the CLIs knock on: a loopback HTTP listener the sessions' hooks
//! post to. It is how Luna learns the exact moment a Claude Code turn starts,
//! ends, or stops for a permission — without polling the registry and without
//! a word reaching the model (the hook commands print nothing, and nothing
//! silent costs context). One hook speaks: the guard's (`/guard`, guard.rs),
//! which refuses a shell call that would wait forever, and says why.
//!
//! Loopback only, on a port picked at start; every request carries its chat's
//! secret, derived from a key minted for this run, so a stray local process
//! cannot feed the activity tracker, and a session that knows its own hook
//! URL cannot post as another chat. Each session's hook file names the port
//! and its secret, and is rewritten at spawn, so a session that outlives a Luna restart simply stops
//! being heard and the registry poll covers it.
//!
//! The HTTP around it is Luna's own, the head parsed by `httparse`, and only
//! as much of it as the clients speak: one request per connection, its body
//! by `Content-Length` alone. curl's hooks (`--data-binary @-` reads all of
//! stdin first), Claude Code's and Codex's MCP clients all send a whole JSON
//! body of known length, so any `Transfer-Encoding` is refused rather than
//! decoded: no chunk parser, and no request whose length two readers could
//! see differently. That way nothing is read before Luna bounds it
//! (admit.rs): a connection that has not yet shown its secret has a slot of
//! `Limits::unknown`, `HEAD_DEADLINE` to send its head and `HEAD_MAX` bytes
//! for it.

use crate::admit::{Deadline, Slot, Slots};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::OnceLock;
use std::time::Duration;

struct Listener {
    port: u16,
    /// What each chat's secret is derived from (`chat_secret`); never in a
    /// URL itself.
    key: String,
}

static LISTENER: OnceLock<Listener> = OnceLock::new();

/// A request's line and headers, the most any client here sends by far.
const HEAD_MAX: usize = 16 * 1024;
/// For the head, from the connection on: a hook's curl sends it at once.
const HEAD_DEADLINE: Duration = Duration::from_secs(5);
/// For a body once the request has shown its secret: megabytes on loopback.
const BODY_DEADLINE: Duration = Duration::from_secs(30);
/// For the whole answer: a client that stops reading does not hold its
/// thread.
const WRITE_DEADLINE: Duration = Duration::from_secs(30);
/// For a 503 written from the accept loop, which must not wait on anyone.
const TURN_AWAY_DEADLINE: Duration = Duration::from_millis(100);
/// For reading away what a client still sends after its answer.
const DRAIN_DEADLINE: Duration = Duration::from_millis(200);
/// Headers in a request: curl and the MCP clients send about ten.
const HEADERS_MAX: usize = 64;
/// A hook's stdin: a small JSON object.
const HOOK_BODY_MAX: usize = 64 * 1024;
/// An MCP call or the shell call the guard is asked about.
const BODY_MAX: usize = 4 * 1024 * 1024;

/// How many connections each stage holds at once. Separate lanes, because an
/// MCP `wait` holds its request for minutes, and a hook must not queue
/// behind it: the hooks' lane is for requests answered in microseconds.
#[derive(Clone, Copy)]
struct Limits {
    /// Connections still sending their head: nobody known yet.
    unknown: usize,
    /// Hooks and the guard, past their secret.
    hooks: usize,
    /// MCP calls, past their token: a `wait` per helper waited on, with room.
    mcp: usize,
    head_deadline: Duration,
}

const LIMITS: Limits = Limits { unknown: 64, hooks: 32, mcp: 128, head_deadline: HEAD_DEADLINE };

/// Starts the listener on its own thread. Called once from setup; a failure
/// to bind is logged and the app runs on the registry poll alone.
pub fn start() {
    let listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(l) => l,
        Err(e) => {
            crate::log::error("hub", &format!("cannot listen on loopback, hooks are off: {e}"));
            return;
        }
    };
    let Ok(addr) = listener.local_addr() else {
        crate::log::error("hub", "listener has no address; hooks are off");
        return;
    };
    if LISTENER.set(Listener { port: addr.port(), key: mint_key() }).is_err() {
        // Started already: the tests start it from more than one place.
        return;
    }
    crate::log::info("hub", &format!("listening on {addr}"));
    std::thread::Builder::new().name("luna-hub".into()).spawn(move || serve(listener, LIMITS)).expect("spawn hub thread");
}

/// The URL a session's hooks post to, or None while the listener is off.
pub fn hook_url(chat_id: &str) -> Option<String> {
    let l = LISTENER.get()?;
    Some(format!("http://127.0.0.1:{}/hook/{}/{}", l.port, chat_id, chat_secret(&l.key, chat_id)))
}

/// The URL a session's guard hook posts a shell call to (guard.rs), or None
/// while the listener is off.
pub fn guard_url(chat_id: &str) -> Option<String> {
    let l = LISTENER.get()?;
    Some(format!("http://127.0.0.1:{}/guard/{}/{}", l.port, chat_id, chat_secret(&l.key, chat_id)))
}

/// The secret of one chat's hook and guard URLs: HMAC-SHA256 of its id under
/// the run's key, 128 bits of it. One chat's URL, which its own session can
/// read, says nothing of another's.
fn chat_secret(key: &str, chat_id: &str) -> String {
    let mut secret = crate::secret::hmac_hex(key.as_bytes(), chat_id.as_bytes());
    secret.truncate(32);
    secret
}

/// The MCP endpoint, or None while the listener is off.
pub fn mcp_url() -> Option<String> {
    let l = LISTENER.get()?;
    Some(format!("http://127.0.0.1:{}/mcp", l.port))
}

/// Accepts for good. A thread per connection, up to the limits: a request
/// past them is answered 503 and closed at once.
fn serve(listener: TcpListener, limits: Limits) {
    let lanes = (Slots::new(limits.unknown), Slots::new(limits.hooks), Slots::new(limits.mcp));
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let Some(slot) = lanes.0.take() else {
            reply(&stream, 503, None, TURN_AWAY_DEADLINE);
            continue;
        };
        let (hooks, mcp) = (lanes.1.clone(), lanes.2.clone());
        let spawned = std::thread::Builder::new().spawn(move || {
            // Whatever goes wrong before an answer, the connection is closed
            // with nothing said: a stray client learns nothing from it.
            let _ = connection(&stream, slot, &hooks, &mcp, limits.head_deadline);
        });
        if spawned.is_err() {
            crate::log::warn("hub", "cannot start a thread for a request");
        }
    }
}

/// What a request asks for, known from its head.
enum Route {
    Hook(String),
    Guard,
    Mcp(String),
}

fn connection(stream: &TcpStream, unknown: Slot, hooks: &Slots, mcp: &Slots, head_deadline: Duration) -> io::Result<()> {
    let mut reader = BufReader::new(Deadline::new(stream, head_deadline));
    let head = match read_head(&mut reader) {
        Ok(head) => head,
        // A head that parses but frames its body in a way Luna does not take:
        // said so, and closed.
        Err(e) if e.kind() == io::ErrorKind::Unsupported => {
            reply(stream, 400, None, WRITE_DEADLINE);
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    let route = match route(&head) {
        Ok(route) => route,
        Err((status, body)) => {
            answer(stream, status, body.as_deref());
            return Ok(());
        }
    };
    let lane = match route {
        Route::Mcp(_) => mcp,
        Route::Hook(_) | Route::Guard => hooks,
    };
    let Some(_busy) = lane.take() else {
        answer(stream, 503, None);
        return Ok(());
    };
    drop(unknown);

    let max = if matches!(route, Route::Hook(_)) { HOOK_BODY_MAX } else { BODY_MAX };
    reader.get_mut().extend(BODY_DEADLINE);
    if head.header("expect").is_some_and(|v| v.eq_ignore_ascii_case("100-continue")) {
        Deadline::new(stream, WRITE_DEADLINE).write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
    }
    let body = match read_body(&mut reader, &head, max) {
        Ok(body) => String::from_utf8(body).unwrap_or_default(),
        Err(e) if e.kind() == io::ErrorKind::FileTooLarge => {
            answer(stream, 413, None);
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    let (status, json) = match route {
        Route::Mcp(caller) => match crate::mcp::handle(&caller, &body) {
            Some(v) => (200, Some(v.to_string())),
            None => (202, None),
        },
        Route::Guard => match crate::guard::decision(&body) {
            Some(decision) => (200, Some(decision)),
            None => (204, None),
        },
        Route::Hook(chat_id) => (on_hook(&chat_id, &body), None),
    };
    answer(stream, status, json.as_deref());
    Ok(())
}

/// The checks a request meets before its body is read: the method, the path
/// and its secret or token, in that order.
fn route(head: &Head) -> Result<Route, (u16, Option<String>)> {
    let path = head.path.as_str();
    let post = head.method == "POST";
    if path == "/mcp" || path.starts_with("/mcp?") {
        // GET is refused (no server-initiated stream is offered), which the
        // transport allows.
        if !post {
            return Err((405, None));
        }
        let token = head.header("authorization").and_then(|v| v.strip_prefix("Bearer ")).unwrap_or_default();
        return match crate::agents::caller_of(token.trim()) {
            Some(caller) => Ok(Route::Mcp(caller)),
            None => Err((401, Some(r#"{"error":"unknown or expired session token"}"#.into()))),
        };
    }
    let guard = path.starts_with("/guard/");
    if !post {
        return Err((405, None));
    }
    let (chat_id, secret) = parse_path(path, if guard { "/guard/" } else { "/hook/" }).ok_or((404, None))?;
    let Some(l) = LISTENER.get() else { return Err((503, None)) };
    // The secret of the chat the path names, so it is that chat's events or
    // none: the guard's too, though it answers for no chat in particular.
    if !crate::secret::same(secret, &chat_secret(&l.key, chat_id)) {
        return Err((403, None));
    }
    Ok(if guard { Route::Guard } else { Route::Hook(chat_id.to_string()) })
}

/// A hook's post: its stdin, forwarded as the body.
fn on_hook(chat_id: &str, body: &str) -> u16 {
    crate::agents::on_hook(chat_id, body);
    match crate::activity::HookEvent::parse(body) {
        Some(ev) => {
            crate::activity::on_hook(chat_id, ev);
            204
        }
        None => 400,
    }
}

struct Head {
    method: String,
    path: String,
    /// Names lowercased, values as text.
    headers: Vec<(String, String)>,
    /// The body's length: `Content-Length`, or 0 without one.
    content_length: usize,
}

impl Head {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
}

fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_string())
}

/// A request Luna understands but does not take: answered 400.
fn unsupported(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, what.to_string())
}

/// The request line and headers, up to `HEAD_MAX` bytes, parsed by
/// `httparse`; what follows them stays in the reader. An error of kind
/// `Unsupported` for a body framed other than by one `Content-Length`.
fn read_head(r: &mut impl BufRead) -> io::Result<Head> {
    let mut raw = Vec::new();
    loop {
        let left = HEAD_MAX.saturating_sub(raw.len()) as u64;
        if left == 0 {
            return Err(bad("head too large"));
        }
        if (&mut *r).take(left).read_until(b'\n', &mut raw)? == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "head cut short"));
        }
        if raw.ends_with(b"\r\n\r\n") || raw.ends_with(b"\n\n") {
            break;
        }
    }
    let mut slots = [httparse::EMPTY_HEADER; HEADERS_MAX];
    let mut req = httparse::Request::new(&mut slots);
    // Strict as httparse is by default: no space before a header's colon,
    // no folded lines, a request line of three parts and HTTP/1.x.
    match req.parse(&raw) {
        Ok(httparse::Status::Complete(n)) if n == raw.len() => {}
        _ => return Err(bad("not an HTTP/1 request head")),
    }
    let (Some(method), Some(path)) = (req.method, req.path) else { return Err(bad("no request line")) };
    let mut headers = Vec::with_capacity(req.headers.len());
    for h in req.headers.iter() {
        let value = std::str::from_utf8(h.value).map_err(|_| bad("a header that is not text"))?;
        headers.push((h.name.to_ascii_lowercase(), value.trim().to_string()));
    }
    if headers.iter().any(|(n, _)| n == "transfer-encoding") {
        return Err(unsupported("transfer-encoding"));
    }
    let mut content_length = None;
    for (_, v) in headers.iter().filter(|(n, _)| n == "content-length") {
        // Digits only: `str::parse` would take a leading `+`.
        if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
            return Err(unsupported("content-length"));
        }
        let n = v.parse::<usize>().map_err(|_| unsupported("content-length"))?;
        if content_length.is_some_and(|m| m != n) {
            return Err(unsupported("conflicting content-length"));
        }
        content_length = Some(n);
    }
    Ok(Head { method: method.to_string(), path: path.to_string(), headers, content_length: content_length.unwrap_or(0) })
}

/// The body of `Content-Length` bytes, refused past `max` with an error of
/// kind `FileTooLarge` before any of it is read.
fn read_body(r: &mut impl BufRead, head: &Head, max: usize) -> io::Result<Vec<u8>> {
    if head.content_length > max {
        return Err(io::Error::new(io::ErrorKind::FileTooLarge, "body too large"));
    }
    let mut body = vec![0u8; head.content_length];
    r.read_exact(&mut body)?;
    Ok(body)
}

/// `reply`, then what the client may still be sending read away until one
/// deadline for all of it: closed on unread data, the socket would reset, and
/// the client might lose the answer.
fn answer(stream: &TcpStream, status: u16, json: Option<&str>) {
    reply(stream, status, json, WRITE_DEADLINE);
    let _ = Deadline::new(stream, DRAIN_DEADLINE).take(64 * 1024).read_to_end(&mut Vec::new());
}

/// The answer, written within one deadline for all of it, and the connection
/// closed: one request per connection.
fn reply(stream: &TcpStream, status: u16, json: Option<&str>, within: Duration) {
    let reason = match status {
        100 => "Continue",
        200 => "OK",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Content Too Large",
        _ => "Service Unavailable",
    };
    let body = json.unwrap_or_default();
    let kind = if json.is_some() { "Content-Type: application/json\r\n" } else { "" };
    let msg = format!("HTTP/1.1 {status} {reason}\r\n{kind}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    let _ = Deadline::new(stream, within).write_all(msg.as_bytes());
    let _ = stream.shutdown(std::net::Shutdown::Write);
}

/// `<prefix><chat id>/<secret>` → (chat id, secret).
fn parse_path<'a>(path: &'a str, prefix: &str) -> Option<(&'a str, &'a str)> {
    let rest = path.strip_prefix(prefix)?;
    let rest = rest.split('?').next()?;
    let (id, secret) = rest.split_once('/')?;
    (!id.is_empty() && !secret.is_empty() && !secret.contains('/')).then_some((id, secret))
}

/// 256 bits from the OS's CSPRNG, as 64 hex characters.
fn mint_key() -> String {
    crate::secret::random_hex(32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hook_paths() {
        assert_eq!(parse_path("/hook/abc/s3cr3t", "/hook/"), Some(("abc", "s3cr3t")));
        assert_eq!(parse_path("/hook/abc/s3cr3t?x=1", "/hook/"), Some(("abc", "s3cr3t")));
        assert_eq!(parse_path("/hook/abc", "/hook/"), None);
        assert_eq!(parse_path("/hook//s", "/hook/"), None);
        assert_eq!(parse_path("/other/abc/s", "/hook/"), None);
        assert_eq!(parse_path("/guard/abc/s3cr3t", "/guard/"), Some(("abc", "s3cr3t")));
        assert_eq!(parse_path("/guard/abc/s3cr3t", "/hook/"), None);
    }

    fn head(raw: &str) -> io::Result<Head> {
        read_head(&mut io::Cursor::new(raw.as_bytes().to_vec()))
    }

    #[test]
    fn reads_a_head_and_leaves_the_body() {
        let mut r = io::Cursor::new(b"POST /mcp HTTP/1.1\r\nAuthorization: Bearer x\r\nContent-Length: 4\r\n\r\nbodyrest".to_vec());
        let h = read_head(&mut r).unwrap();
        assert_eq!((h.method.as_str(), h.path.as_str()), ("POST", "/mcp"));
        assert_eq!(h.header("authorization"), Some("Bearer x"));
        assert_eq!(read_body(&mut r, &h, 100).unwrap(), b"body");
    }

    #[test]
    fn refuses_a_head_too_large_cut_short_or_not_http() {
        let huge = format!("POST /hook/a/b HTTP/1.1\r\nX: {}\r\n\r\n", "y".repeat(HEAD_MAX));
        assert_eq!(head(&huge).err().unwrap().kind(), io::ErrorKind::InvalidData);
        assert_eq!(head("POST /hook/a/b HTTP/1.1\r\nHost: x\r\n").err().unwrap().kind(), io::ErrorKind::UnexpectedEof);
        for raw in [
            "hello\r\n\r\n",
            "POST / SPDY/3\r\n\r\n",
            "POST / HTTP/1.1 extra\r\n\r\n",
            "POST  / HTTP/1.1\r\n\r\n",
            "POST / HTTP/2.0\r\n\r\n",
            "POST / HTTP/1.1\r\nno colon\r\n\r\n",
            "POST / HTTP/1.1\r\nContent-Length : 4\r\n\r\n",
            "POST / HTTP/1.1\r\n Content-Length: 4\r\n\r\n",
            "POST / HTTP/1.1\r\nHost: x\r\n folded\r\n\r\n",
        ] {
            assert_eq!(head(raw).err().map(|e| e.kind()), Some(io::ErrorKind::InvalidData), "{raw:?}");
        }
        let many: String = (0..HEADERS_MAX + 1).map(|i| format!("X-{i}: y\r\n")).collect();
        assert!(head(&format!("POST / HTTP/1.1\r\n{many}\r\n")).is_err(), "more headers than room for");
    }

    #[test]
    fn a_body_is_framed_by_one_content_length_and_nothing_else() {
        let unsupported = |raw: &str| head(raw).err().map(|e| e.kind()) == Some(io::ErrorKind::Unsupported);
        for raw in [
            "POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n",
            "POST / HTTP/1.1\r\nTransfer-Encoding: gzip, chunked\r\n\r\n",
            "POST / HTTP/1.1\r\nContent-Length: 4\r\nTransfer-Encoding: chunked\r\n\r\n",
            "POST / HTTP/1.1\r\nContent-Length: 4\r\nContent-Length: 5\r\n\r\n",
            "POST / HTTP/1.1\r\nContent-Length: 4, 4\r\n\r\n",
            "POST / HTTP/1.1\r\nContent-Length: +4\r\n\r\n",
            "POST / HTTP/1.1\r\nContent-Length: -4\r\n\r\n",
            "POST / HTTP/1.1\r\nContent-Length: 0x4\r\n\r\n",
            "POST / HTTP/1.1\r\nContent-Length:\r\n\r\n",
            "POST / HTTP/1.1\r\nContent-Length: 99999999999999999999999\r\n\r\n",
        ] {
            assert!(unsupported(raw), "{raw:?}");
        }
        let same_twice = head("POST / HTTP/1.1\r\nContent-Length: 4\r\ncontent-length: 4\r\n\r\n").unwrap();
        assert_eq!(same_twice.content_length, 4);
        assert_eq!(head("POST / HTTP/1.1\r\n\r\n").unwrap().content_length, 0);
    }

    #[test]
    fn reads_a_body_within_its_cap() {
        let h = head("POST / HTTP/1.1\r\nContent-Length: 10\r\n\r\n").unwrap();
        assert_eq!(read_body(&mut io::Cursor::new(b"0123456789".to_vec()), &h, 10).unwrap().len(), 10);
        assert_eq!(read_body(&mut io::Cursor::new(b"0123456789".to_vec()), &h, 9).err().unwrap().kind(), io::ErrorKind::FileTooLarge);
        assert_eq!(read_body(&mut io::Cursor::new(b"01234".to_vec()), &h, 10).err().unwrap().kind(), io::ErrorKind::UnexpectedEof);
    }
}

#[cfg(test)]
mod live {
    use super::*;
    use std::net::SocketAddr;
    use std::time::Instant;

    /// The real listener, a real POST the way curl would make it, and the
    /// activity tracker on the other side.
    #[test]
    fn a_posted_hook_reaches_the_tracker() {
        start();
        let url = hook_url("chat-hub-test").expect("listener up");
        // ureq's own error, large as it is, only ever unwrapped here.
        #[allow(clippy::result_large_err)]
        let post = |body: &str| ureq::post(&url).send_string(body).map(|r| r.status());

        assert_eq!(post(r#"{"hook_event_name":"UserPromptSubmit","session_id":"s"}"#).unwrap(), 204);
        assert_eq!(crate::activity::hook_turn("chat-hub-test"), Some(crate::activity::Turn::Busy));
        assert_eq!(post(r#"{"hook_event_name":"Notification","notification_type":"permission_prompt"}"#).unwrap(), 204);
        assert_eq!(crate::activity::hook_turn("chat-hub-test"), Some(crate::activity::Turn::Waiting));
        // What a permission prompt is for reaches the agent layer too.
        let ask = r#"{"hook_event_name":"PermissionRequest","tool_name":"Bash","tool_input":{"command":"ls"}}"#;
        assert_eq!(post(ask).unwrap(), 204);
        assert_eq!(crate::activity::hook_turn("chat-hub-test"), Some(crate::activity::Turn::Waiting));
        assert_eq!(post(r#"{"hook_event_name":"Stop"}"#).unwrap(), 204);
        assert_eq!(crate::activity::hook_turn("chat-hub-test"), Some(crate::activity::Turn::Idle));

        // A bad secret, a bad body and a bad method are told apart from a good one.
        let (base, _secret) = url.rsplit_once('/').unwrap();
        let bad = format!("{base}/nope");
        assert_eq!(ureq::post(&bad).send_string("{}").unwrap_err().into_response().map(|r| r.status()), Some(403));
        // Its own secret is no key to another chat, the guard's door included.
        let (_, own) = url.rsplit_once('/').unwrap();
        let other = hook_url("chat-hub-other").unwrap();
        assert_ne!(other.rsplit_once('/').unwrap().1, own);
        let forged = format!("{}/{own}", other.rsplit_once('/').unwrap().0);
        let stop = r#"{"hook_event_name":"Stop"}"#;
        assert_eq!(ureq::post(&forged).send_string(stop).unwrap_err().into_response().map(|r| r.status()), Some(403));
        assert_eq!(crate::activity::hook_turn("chat-hub-other"), None, "nothing reached the tracker as that chat");
        let guard = guard_url("chat-hub-other").unwrap().replace(other.rsplit_once('/').unwrap().1, own);
        assert_eq!(ureq::post(&guard).send_string("{}").unwrap_err().into_response().map(|r| r.status()), Some(403));
        assert!(ureq::post(&other).send_string(stop).is_ok_and(|r| r.status() == 204), "its own secret lets it in");
        assert_eq!(post("not json").unwrap_err().into_response().map(|r| r.status()), Some(400));
        assert_eq!(ureq::get(&url).call().unwrap_err().into_response().map(|r| r.status()), Some(405));
        // An MCP call without a token, and one past the hooks' body cap.
        let mcp = mcp_url().unwrap();
        assert_eq!(ureq::post(&mcp).send_string("{}").unwrap_err().into_response().map(|r| r.status()), Some(401));
        let addr: SocketAddr = format!("127.0.0.1:{}", LISTENER.get().unwrap().port).parse().unwrap();
        let path = url.split_once("127.0.0.1").unwrap().1.split_once('/').unwrap().1;
        let big = send(addr, format!("POST /{path} HTTP/1.1\r\nContent-Length: {}\r\n\r\n", HOOK_BODY_MAX + 1).as_bytes());
        assert!(big.starts_with("HTTP/1.1 413"), "{big}");
    }

    /// A listener of its own on the run's secret, with these limits.
    fn listener(limits: Limits) -> SocketAddr {
        start();
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || serve(l, limits));
        addr
    }

    fn hook_path(chat: &str) -> String {
        format!("/hook/{chat}/{}", chat_secret(&LISTENER.get().unwrap().key, chat))
    }

    /// What the listener answers to `bytes`, the connection's write side
    /// closed after them: "" for nothing at all.
    fn send(addr: SocketAddr, bytes: &[u8]) -> String {
        let mut c = TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let _ = c.write_all(bytes);
        let _ = c.shutdown(std::net::Shutdown::Write);
        let mut out = Vec::new();
        let _ = c.read_to_end(&mut out);
        String::from_utf8_lossy(&out).into_owned()
    }

    fn post_hook(addr: SocketAddr, chat: &str, body: &str) -> String {
        send(addr, format!("POST {} HTTP/1.1\r\nContent-Length: {}\r\n\r\n{body}", hook_path(chat), body.len()).as_bytes())
    }

    const PROMPT: &str = r#"{"hook_event_name":"UserPromptSubmit","session_id":"s"}"#;

    #[test]
    fn what_is_cut_short_or_too_large_gets_nothing() {
        let addr = listener(LIMITS);
        assert_eq!(send(addr, &[b'A'; HEAD_MAX + 10]), "", "a head past its cap");
        assert_eq!(send(addr, b"POST /hook/a/b HTTP/1.1\r\nHost: x\r\n"), "", "a head cut short");
        let cut = format!("POST {} HTTP/1.1\r\nContent-Length: 100\r\n\r\n{{\"hook", hook_path("chat-hub-cut"));
        assert_eq!(send(addr, cut.as_bytes()), "", "a body cut short");
        assert_eq!(crate::activity::hook_turn("chat-hub-cut"), None, "and nothing reached the tracker");
        let huge = format!("POST {} HTTP/1.1\r\nContent-Length: {}\r\n\r\n", hook_path("chat-hub-cut"), BODY_MAX * 2);
        assert!(send(addr, huge.as_bytes()).starts_with("HTTP/1.1 413"));
    }

    #[test]
    fn a_transfer_encoding_is_refused_before_anything_is_heard() {
        let addr = listener(LIMITS);
        let body = format!("{:x}\r\n{PROMPT}\r\n0\r\n\r\n", PROMPT.len());
        for te in ["Transfer-Encoding: chunked", "Transfer-Encoding: chunked\r\nContent-Length: 5"] {
            let req = format!("POST {} HTTP/1.1\r\n{te}\r\n\r\n{body}", hook_path("chat-hub-te"));
            let out = send(addr, req.as_bytes());
            assert!(out.is_empty() || out.starts_with("HTTP/1.1 400"), "{out}");
        }
        let req = format!("POST {} HTTP/1.1\r\nContent-Length: +5\r\n\r\n{PROMPT}", hook_path("chat-hub-te"));
        let out = send(addr, req.as_bytes());
        assert!(out.is_empty() || out.starts_with("HTTP/1.1 400"), "{out}");
        assert_eq!(crate::activity::hook_turn("chat-hub-te"), None, "nothing reached the tracker");
    }

    #[test]
    fn a_hook_waiting_on_continue_is_heard() {
        let addr = listener(LIMITS);
        let req = format!(
            "POST {} HTTP/1.1\r\nContent-Length: {}\r\nExpect: 100-continue\r\n\r\n{PROMPT}",
            hook_path("chat-hub-continue"),
            PROMPT.len()
        );
        let out = send(addr, req.as_bytes());
        assert!(out.starts_with("HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 204"), "{out}");
        assert_eq!(crate::activity::hook_turn("chat-hub-continue"), Some(crate::activity::Turn::Busy));
    }

    #[test]
    fn a_trickling_stranger_is_let_go_after_its_answer() {
        let addr = listener(LIMITS);
        let mut c = TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        // A bad secret is answered 403, then drained: a byte every 100 ms,
        // each inside any per-read timeout, must not keep the drain going.
        // The listener's end is gone once a write to it fails.
        c.write_all(b"POST /hook/x/wrong HTTP/1.1\r\nContent-Length: 1000000\r\n\r\n").unwrap();
        let mut out = [0u8; 12];
        c.read_exact(&mut out).unwrap();
        assert_eq!(&out, b"HTTP/1.1 403");
        let begun = Instant::now();
        let mut closed = None;
        for _ in 0..100 {
            if c.write_all(b"x").is_err() {
                closed = Some(begun.elapsed());
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let closed = closed.expect("the listener kept reading for ten seconds");
        assert!(closed < Duration::from_secs(3), "{closed:?}");
    }

    #[test]
    fn a_reader_that_stops_reading_is_let_go_at_the_deadline() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        // Connected and never read from: what is written fills the buffers.
        let c = TcpStream::connect(addr).unwrap();
        let (s, _) = l.accept().unwrap();
        let big = "x".repeat(64 * 1024 * 1024);
        let begun = Instant::now();
        reply(&s, 200, Some(&big), Duration::from_millis(500));
        assert!(begun.elapsed() < Duration::from_secs(3), "{:?}", begun.elapsed());
        drop(c);
    }

    #[test]
    fn an_idle_connection_is_let_go_at_the_deadline() {
        let addr = listener(Limits { head_deadline: Duration::from_millis(300), ..LIMITS });
        let begun = Instant::now();
        let mut c = TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut out = Vec::new();
        let _ = c.read_to_end(&mut out);
        assert!(out.is_empty());
        assert!(begun.elapsed() < Duration::from_secs(3), "{:?}", begun.elapsed());
    }

    #[test]
    fn hooks_are_heard_past_idle_strangers() {
        let addr = listener(LIMITS);
        let idle: Vec<TcpStream> = (0..LIMITS.unknown / 2).map(|_| TcpStream::connect(addr).unwrap()).collect();
        let begun = Instant::now();
        assert!(post_hook(addr, "chat-hub-busy", PROMPT).starts_with("HTTP/1.1 204"));
        assert!(begun.elapsed() < Duration::from_secs(2), "{:?}", begun.elapsed());
        drop(idle);
    }

    #[test]
    fn saturation_is_turned_away_and_passes() {
        let addr = listener(Limits { unknown: 2, head_deadline: Duration::from_secs(2), ..LIMITS });
        let idle: Vec<TcpStream> = (0..2).map(|_| TcpStream::connect(addr).unwrap()).collect();
        // The accept loop takes connections in the order they came, so the
        // strangers hold both slots when it reaches this one, as long as their
        // deadline has not passed. It answers without reading, so this one
        // reads before it sends anything: a request left unread when the 503
        // is closed has the close reset the connection, and on Windows a reset
        // drops the answer the client has not read yet.
        let mut c = TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut out = String::new();
        let _ = c.read_to_string(&mut out);
        assert!(out.starts_with("HTTP/1.1 503"), "over the cap: turned away at once: {out:?}");
        // The strangers' deadline passes and their slots come back.
        let begun = Instant::now();
        while !post_hook(addr, "chat-hub-full", PROMPT).starts_with("HTTP/1.1 204") {
            assert!(begun.elapsed() < Duration::from_secs(10), "the slots never came back");
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(begun.elapsed() >= Duration::from_secs(1), "the strangers kept their slots: {:?}", begun.elapsed());
        drop(idle);

        let addr = listener(Limits { hooks: 0, ..LIMITS });
        assert!(post_hook(addr, "chat-hub-full", PROMPT).starts_with("HTTP/1.1 503"), "a full lane turns away too");
    }
}
