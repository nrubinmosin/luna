//! A Codex rollout (`<CODEX_HOME>/sessions/…/rollout-*.jsonl`) as Luna reads
//! it, made of whatever bytes: a prompt-injected agent decides much of what
//! lands there. Seeds: fixtures/transcripts/codex/.
#![no_main]

use libfuzzer_sys::fuzz_target;
use luna_fuzz::{assert_text_from_input, complete_end, from_input, reshaped, strings_in};
use luna_lib::transcripts::{read_rollout, RolloutRead};

fuzz_target!(|data: &[u8]| {
    let read = read_rollout(data, 0);
    assert_eq!(read.consumed, complete_end(data), "a reader stops past the last complete line");

    let strings = strings_in(data);
    for m in &read.messages {
        assert!(m.role == "user" || m.role == "assistant", "{:?}", m.role);
        if m.role == "user" {
            assert!(!m.text.starts_with('<') && !m.text.starts_with("# AGENTS.md"), "context Codex injects: {:?}", m.text);
        }
        assert_text_from_input(&m.text, &strings, |_| false);
    }
    let lines = data.iter().filter(|b| **b == b'\n').count();
    assert!(read.completed as usize <= lines, "{} turns ended in {lines} lines", read.completed);
    if let Some((_, ask)) = &read.ask {
        assert!(!ask.questions.is_empty());
        for q in &ask.questions {
            assert!(!q.text.is_empty() && q.text == q.text.trim() && from_input(&q.text, &strings), "{:?}", q.text);
            assert!(q.options.iter().all(|o| from_input(o, &strings)), "{:?}", q.options);
        }
    }
    if let Some(why) = &read.failed {
        let aborted = why == "its turn was aborted"
            || why.strip_prefix("its turn was aborted (").and_then(|r| r.strip_suffix(')')).is_some_and(|r| from_input(r, &strings));
        assert!(aborted || from_input(why, &strings), "{why:?}");
    }
    let named =
        [&read.cwd, &read.spawned_by, &read.tail.model, &read.last_model, &read.answered_model, &read.last_said, &read.last_said_at];
    for said in named.into_iter().flatten() {
        assert!(from_input(said, &strings), "{said:?} is in no string of the input");
    }
    if let Some(tokens) = read.tail.context_tokens {
        assert!(tokens > 0.0);
    }
    assert!(read.tail.context_window.is_none() || read.tail.context_tokens.is_some());

    // When the process was spawned bears on which turn ends count, and on
    // nothing else read.
    let later = read_rollout(data, i64::MAX);
    assert!(later.completed <= read.completed);
    assert_eq!(RolloutRead { completed: 0, working: None, ..later }, RolloutRead { completed: 0, working: None, ..read });

    let [one_line, nested] = reshaped(data);
    let read = read_rollout(&one_line, 0);
    assert!(read.consumed == one_line.len() && read.messages.len() <= 1 && read.completed <= 1);
    let read = read_rollout(&nested, 0);
    assert!(read.messages.is_empty() && read.completed == 0, "nothing that deep is JSON to serde_json");
});
