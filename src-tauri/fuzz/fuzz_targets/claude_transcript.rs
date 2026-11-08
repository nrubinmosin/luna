//! A Claude Code transcript (`<config>/projects/<cwd>/<sid>.jsonl`) as Luna
//! reads it, made of whatever bytes: a prompt-injected agent decides much of
//! what lands there. Seeds: fixtures/transcripts/claude/.
#![no_main]

use libfuzzer_sys::fuzz_target;
use luna_fuzz::{assert_text_from_input, complete_end, from_input, reshaped, strings_in};
use luna_lib::transcripts::{read_transcript, Message};

fuzz_target!(|data: &[u8]| {
    let read = read_transcript(data);
    assert_eq!(read.consumed, complete_end(data), "a reader stops past the last complete line");

    let strings = strings_in(data);
    // A tool call is folded to a line of the reader's own, `[tool: <name>]`,
    // whose name may hold newlines of its own.
    let tool_line = |line: &str| strings.iter().map(String::as_str).chain(["?"]).any(|name| format!("[tool: {name}]").contains(line));
    for m in &read.messages {
        assert_said(m, &strings, |_| false);
    }
    for m in &read.with_tools {
        assert_said(m, &strings, tool_line);
    }
    // Calls only add to what a line says, so a line that is a message
    // without them is one with them too.
    assert!(read.with_tools.len() >= read.messages.len());

    for title in [&read.custom_title, &read.ai_title].into_iter().flatten() {
        assert!(!title.is_empty() && title == title.trim() && !title.chars().any(char::is_control), "{title:?}");
        assert!(from_input(title, &strings), "{title:?} is in no string of the input");
    }
    if let Some(ai) = &read.ai_title {
        let handle = ai.contains('-') && ai.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        assert!(!handle, "a plan's handle {ai:?} is no title");
    }
    if let Some(prompt) = &read.first_prompt {
        assert!(prompt.chars().count() <= 49 && !prompt.chars().any(char::is_control), "{prompt:?}");
    }
    if let Some((tokens, _)) = &read.usage {
        assert!(*tokens > 0.0);
    }
    for model in read.model.iter().chain(read.usage.as_ref().map(|(_, m)| m).filter(|m| !m.is_empty())) {
        assert!(from_input(model, &strings), "{model:?} is in no string of the input");
    }
    if let Some((name, _)) = &read.pending_tool {
        assert!(name.is_empty() || from_input(name, &strings), "{name:?} is in no string of the input");
    }
    if let Some(Some((said, at))) = &read.last_said {
        assert!(said.split('\n').all(|l| from_input(l, &strings)), "{said:?}");
        assert!(at.as_ref().is_none_or(|at| from_input(at, &strings)));
    }

    // Read on from wherever a read of the start stopped, the conversation is
    // the one a read from the top gets; only where the CLI's own prompt and
    // its answer fall either side of the cut does `after_cli` differ (each
    // read starts afresh).
    let cut = data.len() / 2;
    let head = read_transcript(&data[..cut]);
    let rest = read_transcript(&data[head.consumed..]);
    let pieced: Vec<(&str, &str)> = head.messages.iter().chain(&rest.messages).map(|m| (m.role.as_str(), m.text.as_str())).collect();
    let whole: Vec<(&str, &str)> = read.messages.iter().map(|m| (m.role.as_str(), m.text.as_str())).collect();
    assert_eq!(pieced, whole, "read on from the cursor at {}", head.consumed);

    let [one_line, nested] = reshaped(data);
    let read = read_transcript(&one_line);
    assert!(read.consumed == one_line.len() && read.messages.len() <= 1 && read.with_tools.len() <= 1);
    let read = read_transcript(&nested);
    assert!(read.messages.is_empty() && read.with_tools.is_empty(), "nothing that deep is JSON to serde_json");
});

fn assert_said(m: &Message, strings: &[String], extra: impl Fn(&str) -> bool) {
    assert!(m.role == "user" || m.role == "assistant", "{:?}", m.role);
    assert!(!m.text.starts_with("<local-command") && !m.text.starts_with("<command-name"), "{:?}", m.text);
    assert_text_from_input(&m.text, strings, extra);
}
