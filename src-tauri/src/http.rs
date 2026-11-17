//! Luna's own HTTP requests: the CLI downloads (cli.rs), the usage and model
//! lookups, the OAuth refresh. They go over the reqwest the updater already
//! brings, with the TLS it sets up: rustls on ring, certificates checked
//! against the OS's trust store. No proxy is taken from the environment or the
//! system, as none ever was; gzip is asked for and undone, as ureq did.
//!
//! A request that carries a credential, an `Authorization` header or a body
//! (the OAuth refresh posts its token in one), follows no redirect: a 3xx comes
//! back as the `Error::Status` it is. reqwest would replay a 307's or 308's
//! body to wherever it points. One that carries none, a download above all,
//! follows them (GitHub's release assets are a redirect away).
//!
//! A blocking face over reqwest's async client: each call blocks its thread on
//! Tauri's runtime. The callers are threads of their own or `spawn_blocking`
//! ones; a call from inside an async task would panic.

use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// The most a whole body read into memory may hold (`text`, `json`); a
/// download streams instead (`each_piece`).
const BODY_MAX: usize = 10 << 20;

/// How long a request may take: to connect, between two reads of the answer,
/// and, where set, from the name lookup to the last byte.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Timeouts {
    pub connect: Duration,
    pub read: Duration,
    pub total: Option<Duration>,
}

/// The clients made so far, by timeouts and whether they follow redirects:
/// each made once, with its verifier, and its connections kept for the next
/// request alike.
fn clients() -> &'static Mutex<HashMap<(Timeouts, bool), reqwest::Client>> {
    static CLIENTS: OnceLock<Mutex<HashMap<(Timeouts, bool), reqwest::Client>>> = OnceLock::new();
    CLIENTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn client(t: Timeouts, follow: bool) -> Result<reqwest::Client, String> {
    let mut made = clients().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(client) = made.get(&(t, follow)) {
        return Ok(client.clone());
    }
    // reqwest is built without a crypto provider of its own and takes the
    // process's default. The updater installs ring the same way, only where
    // none is installed yet, so whichever comes first, it is ring.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let redirects = if follow { reqwest::redirect::Policy::default() } else { reqwest::redirect::Policy::none() };
    let mut builder = reqwest::Client::builder()
        .connect_timeout(t.connect)
        .read_timeout(t.read)
        .redirect(redirects)
        .no_proxy()
        .user_agent(concat!("luna/", env!("CARGO_PKG_VERSION")));
    if let Some(total) = t.total {
        builder = builder.timeout(total);
    }
    // Built on the runtime, as its connector wants a reactor to hand.
    let client = block_on(async { builder.build() }).map_err(|e| describe(&e))?;
    made.insert((t, follow), client.clone());
    Ok(client)
}

/// Requests with these timeouts.
#[derive(Clone, Copy)]
pub struct Agent {
    timeouts: Timeouts,
}

impl Agent {
    /// Makes the clients this agent's requests take (one that follows
    /// redirects, one that does not) on first use, so a client that cannot be
    /// made fails here rather than at the first request.
    pub fn new(t: Timeouts) -> Result<Agent, String> {
        client(t, true)?;
        client(t, false)?;
        Ok(Agent { timeouts: t })
    }

    pub fn get(&self, url: &str) -> Request {
        self.request(reqwest::Method::GET, url)
    }

    pub fn post(&self, url: &str) -> Request {
        self.request(reqwest::Method::POST, url)
    }

    fn request(&self, method: reqwest::Method, url: &str) -> Request {
        Request { timeouts: self.timeouts, method, url: url.to_string(), headers: Vec::new(), body: None }
    }
}

enum Body {
    Json(serde_json::Value),
    #[cfg(test)]
    Text(String),
}

pub struct Request {
    timeouts: Timeouts,
    method: reqwest::Method,
    url: String,
    headers: Vec<(String, String)>,
    body: Option<Body>,
}

impl Request {
    pub fn header(mut self, name: &str, value: &str) -> Request {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    pub fn json(mut self, body: &serde_json::Value) -> Request {
        self.body = Some(Body::Json(body.clone()));
        self
    }

    /// For the hub's tests, which post raw bodies.
    #[cfg(test)]
    pub fn body(mut self, body: impl Into<String>) -> Request {
        self.body = Some(Body::Text(body.into()));
        self
    }

    /// Whether it carries a credential, and so follows no redirect.
    fn credentialed(&self) -> bool {
        self.body.is_some() || self.headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("authorization"))
    }

    /// Sends it and waits for the head of the answer. A status outside
    /// 200-299 is an `Error::Status`, with the answer to read.
    pub fn call(self) -> Result<Response, Error> {
        let client = client(self.timeouts, !self.credentialed()).map_err(Error::Transport)?;
        let mut req = client.request(self.method, &self.url);
        for (name, value) in &self.headers {
            req = req.header(name.as_str(), value.as_str());
        }
        req = match self.body {
            Some(Body::Json(v)) => req.json(&v),
            #[cfg(test)]
            Some(Body::Text(t)) => req.body(t),
            None => req,
        };
        // The future made on the runtime too: reqwest starts the total
        // timeout's timer as it makes it.
        let resp = block_on(async move { req.send().await }).map_err(|e| Error::Transport(describe(&e)))?;
        let resp = Response { inner: resp };
        let status = resp.status();
        if (200..300).contains(&status) {
            Ok(resp)
        } else {
            Err(Error::Status(status, self.url, Box::new(resp)))
        }
    }
}

pub struct Response {
    inner: reqwest::Response,
}

impl Response {
    pub fn status(&self) -> u16 {
        self.inner.status().as_u16()
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.inner.headers().get(name).and_then(|v| v.to_str().ok())
    }

    /// Hands `take` each piece of the body as it comes, until the end or its
    /// first error.
    pub fn each_piece(mut self, mut take: impl FnMut(&[u8]) -> Result<(), String>) -> Result<(), String> {
        while let Some(piece) = block_on(async { self.inner.chunk().await }).map_err(|e| describe(&e))? {
            take(&piece)?;
        }
        Ok(())
    }

    /// The whole body, up to `limit` bytes; past that it is an error.
    pub fn bytes(self, limit: usize) -> Result<Vec<u8>, String> {
        let mut body = Vec::new();
        self.each_piece(|piece| {
            if body.len() + piece.len() > limit {
                return Err(format!("the answer is larger than {limit} bytes"));
            }
            body.extend_from_slice(piece);
            Ok(())
        })?;
        Ok(body)
    }

    pub fn text(self) -> Result<String, String> {
        String::from_utf8(self.bytes(BODY_MAX)?).map_err(|e| e.to_string())
    }

    pub fn json<T: DeserializeOwned>(self) -> Result<T, String> {
        serde_json::from_slice(&self.bytes(BODY_MAX)?).map_err(|e| e.to_string())
    }
}

pub enum Error {
    /// An answer, of this status, for this URL.
    Status(u16, String, Box<Response>),
    /// No answer: the name, the connection, TLS or a timeout failed.
    Transport(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Status(code, url, _) => write!(f, "{url}: status code {code}"),
            Error::Transport(why) => f.write_str(why),
        }
    }
}

/// reqwest's error with its causes: on its own it says only which URL failed
/// ("error sending request for url (...)"), the why is in its sources.
fn describe(e: &reqwest::Error) -> String {
    let mut out = e.to_string();
    let mut cause = std::error::Error::source(e);
    while let Some(c) = cause {
        out.push_str(": ");
        out.push_str(&c.to_string());
        cause = c.source();
    }
    out
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tauri::async_runtime::block_on(f)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// A server that reads one request and answers with `answer`, raw.
    fn serve(answer: &'static str) -> String {
        serve_bytes(answer.as_bytes().to_vec()).0
    }

    /// A server that reads one request, answers with `answer`, and sends the
    /// request's head on, as it came.
    fn serve_bytes(answer: Vec<u8>) -> (String, std::sync::mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let n = conn.read(&mut buf).unwrap_or(0);
            let _ = tx.send(String::from_utf8_lossy(&buf[..n]).into_owned());
            let _ = conn.write_all(&answer);
        });
        (format!("http://{addr}/"), rx)
    }

    /// A listener nothing answers on, nonblocking: once a request has come
    /// back, a connection made to it is already waiting in its backlog, so
    /// `reached` tells at once, with no thread left behind to wait on it.
    fn nobody() -> (String, TcpListener) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        (format!("http://{addr}/elsewhere"), listener)
    }

    fn reached(listener: &TcpListener) -> bool {
        match listener.accept() {
            Ok(_) => true,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => false,
            Err(e) => panic!("{e}"),
        }
    }

    /// A 307 to `to`, which keeps the method and the body.
    fn redirect_to(to: &str) -> String {
        let answer = format!("HTTP/1.1 307 Temporary Redirect\r\nLocation: {to}\r\nContent-Length: 0\r\n\r\n");
        serve_bytes(answer.into_bytes()).0
    }

    fn agent() -> Agent {
        let t = Timeouts { connect: Duration::from_secs(5), read: Duration::from_secs(5), total: Some(Duration::from_secs(10)) };
        Agent::new(t).unwrap()
    }

    #[test]
    fn an_answer_is_read_whole() {
        let url = serve("HTTP/1.1 200 OK\r\nContent-Length: 11\r\nX-Kind: test\r\n\r\n{\"a\": true}");
        let resp = agent().get(&url).call().ok().unwrap();
        assert_eq!(resp.header("x-kind"), Some("test"));
        let v: serde_json::Value = resp.json().unwrap();
        assert_eq!(v["a"], true);
    }

    #[test]
    fn a_status_past_299_is_an_error_with_its_answer() {
        let url = serve("HTTP/1.1 429 Too Many Requests\r\nRetry-After: 7\r\nContent-Length: 2\r\n\r\n{}");
        match agent().get(&url).call() {
            Err(Error::Status(429, at, resp)) => {
                assert_eq!(at, url);
                assert_eq!(resp.header("retry-after"), Some("7"));
                assert_eq!(resp.text().unwrap(), "{}");
            }
            Err(e) => panic!("{e}"),
            Ok(_) => panic!("a 429 passed"),
        }
    }

    #[test]
    fn a_body_past_its_limit_is_refused() {
        let url = serve("HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n0123456789");
        let refused = agent().get(&url).call().ok().unwrap().bytes(9).unwrap_err();
        assert!(refused.contains("larger than 9 bytes"), "{refused}");
    }

    #[test]
    fn a_gzipped_answer_is_asked_for_and_undone() {
        use std::io::Write as _;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(br#"{"a": true}"#).unwrap();
        let body = gz.finish().unwrap();
        let mut answer = format!("HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
        answer.extend_from_slice(&body);
        let (url, sent) = serve_bytes(answer);
        let v: serde_json::Value = agent().get(&url).call().ok().unwrap().json().unwrap();
        assert_eq!(v["a"], true);
        assert!(sent.recv().unwrap().to_ascii_lowercase().contains("accept-encoding: gzip"));
    }

    #[test]
    fn a_credential_is_never_carried_through_a_redirect() {
        // A body (the OAuth refresh posts its token in one), and an
        // Authorization header: the 307 comes back, and nothing reaches the
        // other origin.
        let (elsewhere, listener) = nobody();
        let post = agent().post(&redirect_to(&elsewhere)).json(&serde_json::json!({"refresh_token": "secret"})).call();
        assert!(matches!(post, Err(Error::Status(307, ..))));
        assert!(!reached(&listener), "a body went on to the redirect's target");
        let (elsewhere, listener) = nobody();
        let get = agent().get(&redirect_to(&elsewhere)).header("Authorization", "Bearer secret").call();
        assert!(matches!(get, Err(Error::Status(307, ..))));
        assert!(!reached(&listener), "an Authorization header went on to the redirect's target");
    }

    #[test]
    fn a_request_without_a_credential_follows_a_redirect() {
        let target = serve("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
        let resp = agent().get(&redirect_to(&target)).call().ok().unwrap();
        assert_eq!(resp.text().unwrap(), "ok");
    }

    #[test]
    fn no_answer_says_why() {
        // A port nothing listens on: bound, then let go.
        let addr = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        match agent().get(&format!("http://{addr}/")).call() {
            Err(Error::Transport(why)) => assert!(why.contains(&addr.to_string()) && why.contains(": "), "{why}"),
            Err(e) => panic!("{e}"),
            Ok(_) => panic!("nothing listens there"),
        }
    }
}
