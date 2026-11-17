//! Luna's own HTTP requests: the CLI downloads (cli.rs), the usage and model
//! lookups, the OAuth refresh. They go over the reqwest the updater already
//! brings, with the TLS it sets up: rustls on ring, certificates checked
//! against the OS's trust store. No proxy is taken from the environment or the
//! system, as none ever was.
//!
//! A blocking face over reqwest's async client: each call blocks its thread on
//! Tauri's runtime. The callers are threads of their own or `spawn_blocking`
//! ones; a call from inside an async task would panic.

use serde::de::DeserializeOwned;
use std::fmt;
use std::time::Duration;

/// The most a whole body read into memory may hold (`text`, `json`); a
/// download streams instead (`chunk`).
const BODY_MAX: usize = 10 << 20;

/// How long a request may take: to connect, between two reads of the answer,
/// and, where set, from the name lookup to the last byte.
#[derive(Clone, Copy)]
pub struct Timeouts {
    pub connect: Duration,
    pub read: Duration,
    pub total: Option<Duration>,
}

/// A client with its timeouts; requests made from it share its connections.
#[derive(Clone)]
pub struct Agent {
    client: reqwest::Client,
}

impl Agent {
    pub fn new(t: Timeouts) -> Result<Agent, String> {
        // reqwest is built without a crypto provider of its own and takes the
        // process's default. The updater installs ring the same way, only
        // where none is installed yet, so whichever comes first, it is ring.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut builder = reqwest::Client::builder()
            .connect_timeout(t.connect)
            .read_timeout(t.read)
            .no_proxy()
            .user_agent(concat!("luna/", env!("CARGO_PKG_VERSION")));
        if let Some(total) = t.total {
            builder = builder.timeout(total);
        }
        // Built on the runtime, as its connector wants a reactor to hand.
        block_on(async { builder.build() }).map(|client| Agent { client }).map_err(|e| describe(&e))
    }

    pub fn get(&self, url: &str) -> Request {
        Request { url: url.to_string(), inner: self.client.get(url) }
    }

    pub fn post(&self, url: &str) -> Request {
        Request { url: url.to_string(), inner: self.client.post(url) }
    }
}

pub struct Request {
    url: String,
    inner: reqwest::RequestBuilder,
}

impl Request {
    pub fn header(self, name: &str, value: &str) -> Request {
        Request { url: self.url, inner: self.inner.header(name, value) }
    }

    pub fn json(self, body: &serde_json::Value) -> Request {
        Request { url: self.url, inner: self.inner.json(body) }
    }

    /// For the hub's tests, which post raw bodies.
    #[cfg(test)]
    pub fn body(self, body: impl Into<String>) -> Request {
        Request { url: self.url, inner: self.inner.body(body.into()) }
    }

    /// Sends it and waits for the head of the answer. A status outside
    /// 200-299 is an `Error::Status`, with the answer to read.
    pub fn call(self) -> Result<Response, Error> {
        let url = self.url;
        // The future made on the runtime too: reqwest starts the total
        // timeout's timer as it makes it.
        let inner = self.inner;
        let resp = block_on(async move { inner.send().await }).map_err(|e| Error::Transport(describe(&e)))?;
        let resp = Response { inner: resp };
        let status = resp.status();
        if (200..300).contains(&status) {
            Ok(resp)
        } else {
            Err(Error::Status(status, url, Box::new(resp)))
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

    /// The next piece of the body, `None` once it has all come.
    pub fn chunk(&mut self) -> Result<Option<Vec<u8>>, String> {
        block_on(async { self.inner.chunk().await }).map(|c| c.map(|b| b.to_vec())).map_err(|e| describe(&e))
    }

    /// The whole body, up to `limit` bytes; past that it is an error.
    pub fn bytes(mut self, limit: usize) -> Result<Vec<u8>, String> {
        let mut body = Vec::new();
        while let Some(piece) = self.chunk()? {
            if body.len() + piece.len() > limit {
                return Err(format!("the answer is larger than {limit} bytes"));
            }
            body.extend_from_slice(&piece);
        }
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
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = conn.read(&mut buf);
            let _ = conn.write_all(answer.as_bytes());
        });
        format!("http://{addr}/")
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
