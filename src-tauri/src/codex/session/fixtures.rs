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

/// The same reading off the bytes alone (`read_rollout`).
fn seen_in_memory(bytes: &[u8]) -> Seen {
    let at = |since| read_rollout(bytes, since);
    let (r, now) = (at(0), at(NOW_MS));
    let scanned = |r: RolloutRead| Scanned {
        completed: r.completed,
        working: r.working,
        ask: r.ask,
        failed: r.failed,
        last_said: r.last_said,
        last_said_at: r.last_said_at,
    };
    Seen {
        consumed: r.consumed as u64,
        messages: r.messages.iter().map(|m| (m.role.clone(), m.text.clone())).collect(),
        first_prompt: r.first_prompt.clone(),
        cwd: r.cwd.clone(),
        spawned_by: r.spawned_by.clone(),
        tail_model: r.tail.model.clone(),
        context_tokens: r.tail.context_tokens,
        context_window: r.tail.context_window,
        rate_limits: r.tail.rate_limits.as_ref().map(|(v, at)| (v.to_string(), at.clone())),
        last_model: r.last_model.clone(),
        answered_model: r.answered_model.clone(),
        due_ms: r.due_ms,
        scan_from_epoch: scanned(r),
        scan_from_now: scanned(now),
    }
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

#[test]
fn the_bytes_alone_read_as_the_file_does() {
    for (name, bytes, expected) in FIXTURES {
        assert_eq!(format!("{:#?}\n", seen_in_memory(bytes)), expected, "{name}.jsonl read in memory");
    }
}

/// What the conversation of a rollout comes to as another agent reads it, each
/// message with whether its CLI's word marks it a turn's final answer
/// (`luna_report_get` takes it for the report): off the file, and off its bytes.
fn conversation(name: &str, bytes: &[u8]) -> Vec<(String, String, bool)> {
    let flat = |ms: Vec<crate::agents::Message>| ms.into_iter().map(|m| (m.role, m.text, m.final_answer)).collect::<Vec<_>>();
    let path = std::env::temp_dir().join(format!("luna-codex-conversation-{}-{name}.jsonl", std::process::id()));
    std::fs::write(&path, bytes).unwrap();
    let on_disk = flat(messages_from(&path, 0).0);
    let _ = std::fs::remove_file(&path);
    assert_eq!(on_disk, flat(messages_in(bytes).0), "{name}: the bytes alone read as the file does");
    on_disk
}

const REPORT_0169: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/transcripts/codex/report-0169.jsonl"));
const REPORT_TURN_END: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/transcripts/codex/report-turn-end.jsonl"));

/// A Codex 0.169.0 rollout as it was written (lines taken from one, whole): a
/// turn with a message typed into it mid-turn, one interrupted after its
/// commentary, and one answered at once. The final answers carry `phase`; the
/// commentary does not count as one, nor an interrupted turn's last word.
#[test]
fn a_0_169_rollout_gives_its_final_answers_as_the_reports() {
    let said = conversation("report-0169", REPORT_0169);
    let got: Vec<(&str, &str, bool)> = said.iter().map(|(r, t, f)| (r.as_str(), t.as_str(), *f)).collect();
    assert_eq!(
        got,
        vec![
            ("user", "Run the shell command `Start-Sleep -Seconds 6` (PowerShell), then reply \"slept\".", false),
            ("user", "Also: end your final reply with the word PINEAPPLE.", false),
            ("assistant", "slept PINEAPPLE", true),
            ("user", "Run the shell command `Start-Sleep -Seconds 40` (PowerShell), then reply \"slept\".", false),
            ("assistant", "I\u{2019}ll run the requested PowerShell sleep and report when it finishes.", false),
            ("user", "Reply with exactly: STILL-OK", false),
            ("assistant", "STILL-OK", true),
        ]
    );
}

/// Rollouts whose messages do not say which is final (no `phase`, as before
/// 0.167), or leave one out: the turn's end names it. A message of several
/// blocks is one message; a turn whose `response_item`s hold no assistant
/// message is given the one its end says; of a turn's several messages, only
/// the one the end repeats is final.
#[test]
fn a_turn_s_end_names_its_final_answer_where_its_messages_do_not() {
    let said = conversation("report-turn-end", REPORT_TURN_END);
    let got: Vec<(&str, &str, bool)> = said.iter().map(|(r, t, f)| (r.as_str(), t.as_str(), *f)).collect();
    assert_eq!(
        got,
        vec![
            ("user", "Report on the build.", false),
            ("assistant", "Part one.\nPart two.", true),
            ("user", "And the tests?", false),
            ("assistant", "Tests pass.", true),
            ("user", "Finish up.", false),
            ("assistant", "Working on it.", false),
            ("assistant", "All done: the report.", true),
        ]
    );
}

/// A read from a cursor inside a turn marks what it can see and adds nothing:
/// the message the turn's end repeats may have gone by in the read before.
#[test]
fn a_read_that_opens_inside_a_turn_adds_no_message_of_its_own() {
    let text = std::str::from_utf8(REPORT_TURN_END).unwrap();
    // From turn c's last message on: its start and "Working on it." were read before.
    let from = text.find("{\"timestamp\":\"2026-11-05T09:00:06.000Z\"").unwrap();
    let (rest, _) = messages_in(&REPORT_TURN_END[from..]);
    let got: Vec<(&str, bool)> = rest.iter().map(|m| (m.text.as_str(), m.final_answer)).collect();
    assert_eq!(got, vec![("All done: the report.", true)]);
    // From the end line alone: nothing is added, there being no message here.
    let end = text.find("{\"timestamp\":\"2026-11-05T09:00:06.100Z\"").unwrap();
    assert!(messages_in(&REPORT_TURN_END[end..]).0.is_empty());
}
