//! The door the CLIs knock on: a loopback HTTP listener the sessions' hooks
//! post to. It is how Luna learns the exact moment a Claude Code turn starts,
//! ends, or stops for a permission — without polling the registry and without
//! a word reaching the model (the hook commands print nothing, and nothing
//! silent costs context). One hook speaks: the guard's (`/guard`, guard.rs),
//! which refuses a shell call that would wait forever, and says why.
//!
//! Loopback only, on a port picked at start; every request carries a secret
//! minted for this run, so a stray local process cannot feed the activity
//! tracker. Each session's hook file names the port and the secret, and is
//! rewritten at spawn, so a session that outlives a Luna restart simply stops
//! being heard and the registry poll covers it.
//!
//! The HTTP is Luna's own, and only as much of it as the clients speak: one
//! request per connection, its body by `Content-Length` or chunked. That way
//! nothing is read before Luna bounds it (admit.rs): a connection that has not
//! yet shown its secret has a slot of `Limits::unknown`, `HEAD_DEADLINE` to
//! send its head and `HEAD_MAX` bytes for it.

use crate::admit::{Deadline, Slot, Slots};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::OnceLock;
use std::time::Duration;

struct Listener {
    port: u16,
    secret: String,
}

static LISTENER: OnceLock<Listener> = OnceLock::new();

/// A request's line and headers, the most any client here sends by far.
const HEAD_MAX: usize = 16 * 1024;
/// For the head, from the connection on: a hook's curl sends it at once.
const HEAD_DEADLINE: Duration = Duration::from_secs(5);
/// For a body once the request has shown its secret: megabytes on loopback.
const BODY_DEADLINE: Duration = Duration::from_secs(30);
/// For the answer: a client that stops reading does not hold its thread.
const WRITE_DEADLINE: Duration = Duration::from_secs(30);
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
    if LISTENER.set(Listener { port: addr.port(), secret: mint_secret() }).is_err() {
        // Started already: the tests start it from more than one place.
        return;
    }
    crate::log::info("hub", &format!("listening on {addr}"));
    std::thread::Builder::new().name("luna-hub".into()).spawn(move || serve(listener, LIMITS)).expect("spawn hub thread");
}

/// The URL a session's hooks post to, or None while the listener is off.
pub fn hook_url(chat_id: &str) -> Option<String> {
    let l = LISTENER.get()?;
    Some(format!("http://127.0.0.1:{}/hook/{}/{}", l.port, chat_id, l.secret))
}

/// The URL a session's guard hook posts a shell call to (guard.rs), or None
/// while the listener is off.
pub fn guard_url(chat_id: &str) -> Option<String> {
    let l = LISTENER.get()?;
    Some(format!("http://127.0.0.1:{}/guard/{}/{}", l.port, chat_id, l.secret))
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
            reply(&stream, 503, None);
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
    let head = read_head(&mut reader)?;
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
        let mut s = stream;
        s.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
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
    if !crate::secret::same(secret, &l.secret) {
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
    /// Names lowercased.
    headers: Vec<(String, String)>,
}

impl Head {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
}

fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_string())
}

/// The request line and headers, up to `HEAD_MAX` bytes; what follows them
/// stays in the reader.
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
    let text = std::str::from_utf8(&raw).map_err(|_| bad("head is not text"))?;
    let mut lines = text.lines();
    let mut start = lines.next().unwrap_or_default().split(' ');
    let (Some(method), Some(path), Some(version)) = (start.next(), start.next(), start.next()) else {
        return Err(bad("no request line"));
    };
    if !version.starts_with("HTTP/1.") {
        return Err(bad("not HTTP/1"));
    }
    let headers = lines
        .filter(|l| !l.is_empty())
        .map(|l| l.split_once(':').map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_string())).ok_or_else(|| bad("bad header")))
        .collect::<io::Result<_>>()?;
    Ok(Head { method: method.to_string(), path: path.to_string(), headers })
}

/// The body, by `Content-Length` or chunked, refused past `max` with an
/// error of kind `FileTooLarge` before more of it is read.
fn read_body(r: &mut impl BufRead, head: &Head, max: usize) -> io::Result<Vec<u8>> {
    let too_large = || io::Error::new(io::ErrorKind::FileTooLarge, "body too large");
    if head.header("transfer-encoding").is_some_and(|v| v.eq_ignore_ascii_case("chunked")) {
        let mut body = Vec::new();
        loop {
            let mut line = Vec::new();
            (&mut *r).take(64).read_until(b'\n', &mut line)?;
            let line = std::str::from_utf8(&line).map_err(|_| bad("bad chunk size"))?;
            let hex = line.split(';').next().unwrap_or_default().trim();
            let size = usize::from_str_radix(hex, 16).map_err(|_| bad("bad chunk size"))?;
            if size == 0 {
                // Trailers, if any, up to the empty line.
                loop {
                    let mut t = Vec::new();
                    if (&mut *r).take(HEAD_MAX as u64).read_until(b'\n', &mut t)? == 0 || t == b"\r\n" || t == b"\n" {
                        return Ok(body);
                    }
                }
            }
            if body.len() + size > max {
                return Err(too_large());
            }
            let at = body.len();
            body.resize(at + size, 0);
            r.read_exact(&mut body[at..])?;
            let mut crlf = Vec::new();
            (&mut *r).take(2).read_until(b'\n', &mut crlf)?;
        }
    }
    let len = match head.header("content-length") {
        Some(v) => v.parse::<usize>().map_err(|_| bad("bad content-length"))?,
        None => 0,
    };
    if len > max {
        return Err(too_large());
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    Ok(body)
}

/// `reply`, then what the client may still be sending read away for a
/// moment: closed on unread data, the socket would reset, and the client
/// might lose the answer.
fn answer(stream: &TcpStream, status: u16, json: Option<&str>) {
    reply(stream, status, json);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
    let _ = (&mut &*stream).take(64 * 1024).read_to_end(&mut Vec::new());
}

/// The answer, and the connection closed: one request per connection.
fn reply(stream: &TcpStream, status: u16, json: Option<&str>) {
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
    let _ = stream.set_write_timeout(Some(WRITE_DEADLINE));
    let mut s = stream;
    let _ = s.write_all(msg.as_bytes()).and_then(|()| s.flush());
    let _ = stream.shutdown(std::net::Shutdown::Write);
}

/// `<prefix><chat id>/<secret>` → (chat id, secret).
fn parse_path<'a>(path: &'a str, prefix: &str) -> Option<(&'a str, &'a str)> {
    let rest = path.strip_prefix(prefix)?;
    let rest = rest.split('?').next()?;
    let (id, secret) = rest.split_once('/')?;
    (!id.is_empty() && !secret.is_empty() && !secret.contains('/')).then_some((id, secret))
}

/// 128 bits from the OS's CSPRNG, as 32 hex characters.
fn mint_secret() -> String {
    crate::secret::random_hex(16)
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
        assert!(head("hello\r\n\r\n").is_err());
        assert!(head("POST / SPDY/3\r\n\r\n").is_err());
        assert!(head("POST / HTTP/1.1\r\nno colon\r\n\r\n").is_err());
    }

    #[test]
    fn reads_bodies_by_length_and_chunked_within_their_cap() {
        let h = head("POST / HTTP/1.1\r\nContent-Length: 10\r\n\r\n").unwrap();
        assert_eq!(read_body(&mut io::Cursor::new(b"0123456789".to_vec()), &h, 10).unwrap().len(), 10);
        assert_eq!(read_body(&mut io::Cursor::new(b"0123456789".to_vec()), &h, 9).err().unwrap().kind(), io::ErrorKind::FileTooLarge);
        assert_eq!(read_body(&mut io::Cursor::new(b"01234".to_vec()), &h, 10).err().unwrap().kind(), io::ErrorKind::UnexpectedEof);

        let h = head("POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n").unwrap();
        let chunked = b"4\r\nWiki\r\n5;x=y\r\npedia\r\n0\r\nTrailer: z\r\n\r\n".to_vec();
        assert_eq!(read_body(&mut io::Cursor::new(chunked.clone()), &h, 9).unwrap(), b"Wikipedia");
        assert_eq!(read_body(&mut io::Cursor::new(chunked), &h, 8).err().unwrap().kind(), io::ErrorKind::FileTooLarge);
        assert!(read_body(&mut io::Cursor::new(b"4\r\nWi".to_vec()), &h, 9).is_err());
        assert!(read_body(&mut io::Cursor::new(b"zz\r\n".to_vec()), &h, 9).is_err());
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
        format!("/hook/{chat}/{}", LISTENER.get().unwrap().secret)
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
    fn a_chunked_hook_is_heard() {
        let addr = listener(LIMITS);
        let body = format!("{:x}\r\n{PROMPT}\r\n0\r\n\r\n", PROMPT.len());
        let req =
            format!("POST {} HTTP/1.1\r\nTransfer-Encoding: chunked\r\nExpect: 100-continue\r\n\r\n{body}", hook_path("chat-hub-chunked"));
        let out = send(addr, req.as_bytes());
        assert!(out.contains("HTTP/1.1 204"), "{out}");
        assert_eq!(crate::activity::hook_turn("chat-hub-chunked"), Some(crate::activity::Turn::Busy));
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
        let addr = listener(Limits { unknown: 2, head_deadline: Duration::from_millis(300), ..LIMITS });
        let idle: Vec<TcpStream> = (0..2).map(|_| TcpStream::connect(addr).unwrap()).collect();
        // The accept loop takes them in order: let it reach both.
        std::thread::sleep(Duration::from_millis(100));
        assert!(post_hook(addr, "chat-hub-full", PROMPT).starts_with("HTTP/1.1 503"), "over the cap: turned away at once");
        // The strangers' deadline passes and their slots come back.
        std::thread::sleep(Duration::from_millis(500));
        assert!(post_hook(addr, "chat-hub-full", PROMPT).starts_with("HTTP/1.1 204"));
        drop(idle);

        let addr = listener(Limits { hooks: 0, ..LIMITS });
        assert!(post_hook(addr, "chat-hub-full", PROMPT).starts_with("HTTP/1.1 503"), "a full lane turns away too");
    }
}
