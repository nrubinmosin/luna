//! What both targets check of a reading beyond its not panicking: that it
//! stops where the complete lines do, that every text it hands over comes out
//! of the input, and that a cost growing faster than the input shows.

/// Where a reader of complete lines stops: just past the last newline.
pub fn complete_end(data: &[u8]) -> usize {
    data.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1)
}

/// Every string in the input's lines that parse as JSON, keys and values,
/// and in turn those of a string that is JSON itself (Codex's call arguments,
/// a code cell's `// @exec:` pragma), as the readers decode them.
pub fn strings_in(data: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(data);
    let mut out = Vec::new();
    for line in text.split('\n') {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            collect(&v, 0, &mut out);
        }
    }
    out
}

fn collect(v: &serde_json::Value, depth: usize, out: &mut Vec<String>) {
    match v {
        serde_json::Value::String(s) => {
            out.push(s.clone());
            if depth < 4 {
                for nested in std::iter::once(s.as_str()).chain(s.lines().next().and_then(|l| l.trim().strip_prefix("// @exec:"))) {
                    if let Ok(inner) = serde_json::from_str::<serde_json::Value>(nested) {
                        collect(&inner, depth + 1, out);
                    }
                }
            }
        }
        serde_json::Value::Array(items) => items.iter().for_each(|i| collect(i, depth, out)),
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                out.push(k.clone());
                collect(v, depth, out);
            }
        }
        _ => {}
    }
}

/// Whether `piece` is part of some string the input holds.
pub fn from_input(piece: &str, strings: &[String]) -> bool {
    strings.iter().any(|s| s.contains(piece))
}

/// Asserts each line of a message's text came out of the input: the text is
/// its blocks' texts joined by newlines and trimmed, so each of its lines is
/// part of one block's. `extra` allows the lines a reader writes itself.
pub fn assert_text_from_input(text: &str, strings: &[String], extra: impl Fn(&str) -> bool) {
    assert!(!text.is_empty(), "an empty message");
    assert_eq!(text, text.trim(), "a message not trimmed");
    for line in text.split('\n') {
        assert!(from_input(line, strings) || extra(line), "{line:?} is in no string of the input");
    }
}

/// The input made into the shapes a reader could choke on, each about as long
/// as the input: all of it one line, and that line nested 4096 arrays deep
/// (serde_json gives up at 128). Read in the time libFuzzer gives a unit,
/// they show a cost that grows with a line's length or depth rather than
/// with its bytes.
pub fn reshaped(data: &[u8]) -> [Vec<u8>; 2] {
    let one_line: Vec<u8> = data.iter().map(|b| if *b == b'\n' { b' ' } else { *b }).chain([b'\n']).collect();
    let depth = 4096;
    let nested = [&b"[".repeat(depth)[..], &one_line[..one_line.len() - 1], &b"]".repeat(depth)[..], b"\n"].concat();
    [one_line, nested]
}
