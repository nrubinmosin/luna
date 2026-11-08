//! What Luna reads of each transcript in `fixtures/transcripts/claude/`, held
//! to the `.expected` beside it: the conversation, the opening prompt, the
//! titles and everything the end of a transcript is asked for. The same files
//! seed the fuzz corpus (fuzz/), so a reading changed on purpose shows here as
//! a diff to review. `LUNA_BLESS=1` writes the readings anew, into a checkout
//! the test can write to.

use super::*;

// Built in: the Windows test executables run on a host where the checkout's
// path inside the build container means nothing.
macro_rules! fixture {
    ($name:literal) => {
        (
            $name,
            include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/transcripts/claude/", $name, ".jsonl")),
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/transcripts/claude/", $name, ".expected")),
        )
    };
}

/// Each fixture's name, its bytes and the reading it is held to.
const FIXTURES: [(&str, &[u8], &str); 7] = [
    fixture!("conversation"),
    fixture!("injected"),
    fixture!("api-error"),
    fixture!("tools-due"),
    fixture!("plan-handle"),
    fixture!("cut-short"),
    fixture!("crlf-not-utf8"),
];

/// A reading, every part of it as its caller is given it.
#[derive(Debug, PartialEq)]
struct Seen {
    /// Where a reader from the top stops: the end of the last complete line.
    consumed: u64,
    /// (role, after_cli, text).
    messages: Vec<(String, bool, String)>,
    with_tools: Vec<(String, bool, String)>,
    first_prompt: Option<String>,
    title: Option<(String, bool)>,
    /// (tokens, window).
    context: Option<(f64, f64)>,
    model: Option<String>,
    last_said: Option<String>,
    last_said_at: Option<String>,
    api_error: Option<String>,
    /// (name, input as JSON).
    pending_tool: Option<(String, String)>,
    due_ms: Option<u64>,
}

fn said(msgs: Vec<crate::agents::Message>) -> Vec<(String, bool, String)> {
    msgs.into_iter().map(|m| (m.role, m.after_cli, m.text)).collect()
}

/// The reading through the functions the rest of Luna calls, off a file in
/// an account folder of the test's own.
fn seen_on_disk(n: usize, bytes: &[u8]) -> Seen {
    let root = std::env::temp_dir().join(format!("luna-claude-fixture-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let acct = root.to_string_lossy().into_owned();
    let (cwd, sid) = ("E:\\p", format!("{:08x}-0000-4000-8000-{n:012x}", std::process::id()));
    let path = transcript_file(&acct, cwd, &sid);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, bytes).unwrap();
    let (messages, consumed) = messages_from(&path, 0, false);
    let end = ending(&path);
    let seen = Seen {
        consumed,
        messages: said(messages),
        with_tools: said(messages_from(&path, 0, true).0),
        first_prompt: cached_first_prompt(&acct, cwd, &sid),
        title: saved_title(&acct, &sid).map(|t| (t.title, t.renamed)),
        context: last_context(&path),
        model: last_model(&path),
        last_said: end.last_said,
        last_said_at: end.last_said_at,
        api_error: api_error_of(&path),
        pending_tool: pending_tool(&path).map(|(name, input)| (name, input.to_string())),
        due_ms: due_ms(&path),
    };
    let _ = std::fs::remove_dir_all(&root);
    seen
}

fn held_to(name: &str, seen: &Seen, expected: &str) {
    let got = format!("{seen:#?}\n");
    if std::env::var_os("LUNA_BLESS").is_some() {
        let file = format!("{}/fixtures/transcripts/claude/{name}.expected", env!("CARGO_MANIFEST_DIR"));
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
