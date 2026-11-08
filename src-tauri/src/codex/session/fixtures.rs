//! What Luna reads of each rollout in `fixtures/transcripts/codex/`, held to
//! the `.expected` beside it: the conversation, the opening prompt, the first
//! line, the tail, the models, when a call is due and the running scan. The
//! same files seed the fuzz corpus (fuzz/), so a reading changed on purpose
//! shows here as a diff to review. `LUNA_BLESS=1` writes the readings anew,
//! into a checkout the test can write to.

use super::*;

// Built in: the Windows test executables run on a host where the checkout's
// path inside the build container means nothing.
macro_rules! fixture {
    ($name:literal) => {
        (
            $name,
            include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/transcripts/codex/", $name, ".jsonl")),
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/transcripts/codex/", $name, ".expected")),
        )
    };
}

/// Each fixture's name, its bytes and the reading it is held to.
const FIXTURES: [(&str, &[u8], &str); 6] =
    [fixture!("turn"), fixture!("models"), fixture!("asks"), fixture!("failures"), fixture!("calls-due"), fixture!("helper")];

/// A scan of the whole file, turns counted from `since_ms` on.
#[derive(Debug, PartialEq)]
struct Scanned {
    completed: u32,
    working: Option<bool>,
    ask: Option<(String, crate::agents::Ask)>,
    failed: Option<String>,
    last_said: Option<String>,
    last_said_at: Option<String>,
}

impl From<Scan> for Scanned {
    fn from(s: Scan) -> Self {
        let Scan { completed, working, ask, failed, last_said, last_said_at } = s;
        Scanned { completed, working, ask, failed, last_said, last_said_at }
    }
}

/// A reading, every part of it as its caller is given it.
#[derive(Debug, PartialEq)]
struct Seen {
    /// Where a reader from the top stops: the end of the last complete line.
    consumed: u64,
    /// (role, text).
    messages: Vec<(String, String)>,
    first_prompt: Option<String>,
    cwd: Option<String>,
    spawned_by: Option<String>,
    tail_model: Option<String>,
    context_tokens: Option<f64>,
    context_window: Option<f64>,
    /// (snapshot as JSON, its line's timestamp).
    rate_limits: Option<(String, String)>,
    last_model: Option<String>,
    answered_model: Option<String>,
    due_ms: Option<u64>,
    scan_from_epoch: Scanned,
    /// Every turn end written before the spawn: none counts.
    scan_from_now: Scanned,
}

/// When the scan `scan_from_now` counts from: after every fixture's turns.
const NOW_MS: i64 = 1_900_000_000_000;

/// The reading through the functions the rest of Luna calls, off a file.
fn seen_on_disk(n: usize, bytes: &[u8]) -> Seen {
    let path = std::env::temp_dir().join(format!("luna-codex-fixture-{}-{n}.jsonl", std::process::id()));
    std::fs::write(&path, bytes).unwrap();
    let (messages, consumed) = messages_from(&path, 0);
    let first = first_line(&path);
    let tail = read_tail(&path);
    let scan = |since| {
        let mut s = Scan::default();
        scan_rollout(&path, 0, since, &mut s);
        Scanned::from(s)
    };
    let seen = Seen {
        consumed,
        messages: messages.into_iter().map(|m| (m.role, m.text)).collect(),
        first_prompt: read_first_prompt(&path),
        cwd: first.as_ref().and_then(|v| v["payload"]["cwd"].as_str().map(str::to_owned)),
        spawned_by: first.as_ref().and_then(spawned_by),
        tail_model: tail.model,
        context_tokens: tail.context_tokens,
        context_window: tail.context_window,
        rate_limits: tail.rate_limits.map(|(v, at)| (v.to_string(), at)),
        last_model: last_model(&path),
        answered_model: answered_model(&path),
        due_ms: due_ms(&path),
        scan_from_epoch: scan(0),
        scan_from_now: scan(NOW_MS),
    };
    let _ = std::fs::remove_file(&path);
    seen
}

fn held_to(name: &str, seen: &Seen, expected: &str) {
    let got = format!("{seen:#?}\n");
    if std::env::var_os("LUNA_BLESS").is_some() {
        let file = format!("{}/fixtures/transcripts/codex/{name}.expected", env!("CARGO_MANIFEST_DIR"));
        std::fs::write(file, &got).unwrap();
        return;
    }
    assert_eq!(got, expected, "{name}.jsonl no longer reads as {name}.expected says");
}

#[test]
fn every_fixture_reads_as_it_did() {
    for (n, (name, bytes, expected)) in FIXTURES.iter().enumerate() {
        held_to(name, &seen_on_disk(n, bytes), expected);
    }
}
