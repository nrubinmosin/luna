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

/// The same reading off the bytes alone (`read_transcript`), each part put
/// as its file-based reader gives it.
fn seen_in_memory(bytes: &[u8]) -> Seen {
    let r = read_transcript(bytes);
    let (last_said, last_said_at) = r.last_said.flatten().map_or((None, None), |(said, at)| (Some(said), at));
    Seen {
        consumed: r.consumed as u64,
        messages: said(r.messages),
        with_tools: said(r.with_tools),
        first_prompt: r.first_prompt,
        title: match (r.custom_title, r.ai_title) {
            (Some(title), _) => Some((title, true)),
            (None, ai) => ai.map(|title| (title, false)),
        },
        context: r.usage.map(|(tokens, model)| (tokens, crate::claude::models::context_window(&model))),
        model: r.model,
        last_said,
        last_said_at,
        api_error: r.api_error.flatten().as_ref().map(api_error_text),
        pending_tool: r.pending_tool.map(|(name, input)| (name, input.to_string())),
        due_ms: r.due_ms,
    }
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

#[test]
fn the_bytes_alone_read_as_the_file_does() {
    for (name, bytes, expected) in FIXTURES {
        assert_eq!(format!("{:#?}\n", seen_in_memory(bytes)), expected, "{name}.jsonl read in memory");
    }
}

/// A reader that goes on from its cursor, wherever a read stopped, gets what
/// one read from the top does: the lines are cut in the same places.
#[test]
fn the_lines_in_memory_are_the_lines_on_disk() {
    let path = std::env::temp_dir().join(format!("luna-claude-lines-{}.jsonl", std::process::id()));
    let mut samples: Vec<Vec<u8>> = FIXTURES.iter().map(|(_, bytes, _)| bytes.to_vec()).collect();
    samples.push(b"a\r\r\n\nb\n\r\nc".to_vec());
    for bytes in samples {
        std::fs::write(&path, &bytes).unwrap();
        let (mut on_disk, mut in_memory) = (Vec::new(), Vec::new());
        let read = crate::pty::complete_lines(&path, 0, |l| on_disk.push(l.to_string()));
        assert_eq!(crate::pty::complete_lines_in(&bytes, |l| in_memory.push(l.to_string())) as u64, read);
        assert_eq!(in_memory, on_disk);
        for cut in 0..=bytes.len() {
            let mut lines = Vec::new();
            let first = crate::pty::complete_lines_in(&bytes[..cut], |l| lines.push(l.to_string()));
            crate::pty::complete_lines_in(&bytes[first..], |l| lines.push(l.to_string()));
            assert_eq!(lines, on_disk, "cut at {cut}");
        }
        use std::io::BufRead;
        let buffered: Vec<String> = std::io::BufReader::new(&bytes[..]).lines().map_while(Result::ok).collect();
        assert_eq!(crate::pty::text_lines(&bytes).collect::<Vec<_>>(), buffered);
    }
    let _ = std::fs::remove_file(&path);
}
