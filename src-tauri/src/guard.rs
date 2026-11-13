//! A guard against waits that never end. A shell command that follows a file
//! — `tail -f`, `Get-Content -Wait` — with no deadline of its own outlives the
//! tool that ran it: on Windows a Monitor that expires or a shell that is
//! killed leaves its pipeline behind, and it holds the PC awake all night (as
//! in a campaign on 2026-11-04). Every helper, and a chat whose person ticked
//! "Wait guard" in the new-chat dialog, get a PreToolUse hook that posts the
//! call here (hub.rs `/guard`,
//! pty/start.rs `claude_settings_file` and `codex_guard_args`);
//! such a command is refused with what to do instead, everything else goes
//! ahead. A deadline is a `timeout N`
//! in front of it in the same command; a tool's own `timeout` is none (Claude
//! Code moves a call past it to the background, where it runs on).

/// The tools whose command the guard looks at: Claude Code's `Bash`,
/// `PowerShell` and `Monitor`, and Codex's shell, which its hooks call `Bash`.
pub const TOOLS: &str = "Bash|PowerShell|Monitor";

/// What the model is told when a command is refused. PowerShell's `timeout`
/// only waits, so it is shown a loop that ends.
pub const REFUSAL: &str = "Luna refused this command: it follows a file (`tail -f`, `Get-Content -Wait`) with no \
deadline of its own, and such a wait outlives the tool that runs it and keeps the machine awake. In bash put \
`timeout N` in front of it in the same command (`timeout 1800 tail -f run.log | grep -m1 DONE`); in PowerShell poll \
to a deadline (`$end=(Get-Date).AddMinutes(30); while((Get-Date) -lt $end){ if(Select-String DONE run.log -Quiet){break}; \
Start-Sleep 5 }`). The tool's own timeout is no deadline.";

/// The hook's answer to a call, from its JSON (`tool_name`, `tool_input.command`):
/// Some(the decision to print) for one to refuse, None to let it go ahead. The
/// same shape for both CLIs.
pub fn decision(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let tool = v["tool_name"].as_str()?;
    if !TOOLS.split('|').any(|t| t == tool) {
        return None;
    }
    let command = v["tool_input"]["command"].as_str()?;
    if !follows_unbounded(command) {
        return None;
    }
    crate::log::info("guard", &format!("refused a {tool} call that follows a file with no deadline: {}", one_line(command)));
    Some(
        serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": REFUSAL,
            }
        })
        .to_string(),
    )
}

fn one_line(command: &str) -> String {
    let flat: String = command.split_whitespace().collect::<Vec<_>>().join(" ");
    match flat.char_indices().nth(200) {
        Some((at, _)) => format!("{}…", &flat[..at]),
        None => flat,
    }
}

/// Whether a command, as bash or PowerShell would read it, follows a file in
/// one of its simple commands with no `timeout` in front of it there. A shell
/// started with a command of its own (`bash -c "…"`, `powershell -Command …`)
/// is looked into, its `timeout` counting for what it runs. What sits in quotes
/// as an argument (a commit message, an `echo`), a comment or a heredoc's body
/// is not a command.
pub fn follows_unbounded(command: &str) -> bool {
    simple_commands(command).iter().any(|words| unbounded_in(words))
}

/// A program's name as a command word has it: no folder, no `.exe`, any case.
fn program(word: &str) -> String {
    let w = word.rsplit(['/', '\\']).next().unwrap_or(word).to_ascii_lowercase();
    w.strip_suffix(".exe").map(str::to_owned).unwrap_or(w)
}

/// One simple command: the program it runs, past what may stand in front of it
/// (`X=1`, `$x =`, `nohup`, `stdbuf -oL`, `then`, `do`, …), follows a file with
/// no deadline. A `timeout` in front bounds all of it, unless its duration is
/// zero, which GNU timeout takes for none.
fn unbounded_in(words: &[String]) -> bool {
    let mut i = 0;
    loop {
        let Some(word) = words.get(i) else { return false };
        // `NAME=value` in bash, `$name =` in PowerShell.
        if word.contains('=') && !word.starts_with('-') && !word.starts_with('=') {
            i += 1;
            continue;
        }
        if word.starts_with('$') && words.get(i + 1).is_some_and(|w| w == "=") {
            i += 2;
            continue;
        }
        match program(word).as_str() {
            "timeout" => {
                i += 1;
                while let Some(w) = words.get(i).filter(|w| w.starts_with('-')) {
                    // The options that take a value of their own.
                    i += if matches!(w.as_str(), "-s" | "-k" | "--signal" | "--kill-after") { 2 } else { 1 };
                }
                if !words.get(i).is_some_and(|d| duration_is_zero(d)) {
                    return false;
                }
                i += 1;
            }
            "nohup" | "exec" | "sudo" | "env" | "command" | "stdbuf" | "nice" | "time" => {
                i += 1;
                while words.get(i).is_some_and(|w| w.starts_with('-') || w.chars().all(|c| c.is_ascii_digit())) {
                    i += 1;
                }
            }
            // The shell's own words before a command.
            "if" | "then" | "else" | "elif" | "do" | "while" | "until" | "!" => i += 1,
            _ => break,
        }
    }
    let args = &words[i + 1..];
    match program(&words[i]).as_str() {
        // Its options end at `--`: what follows is a file, `-f` or not.
        "tail" => args.iter().take_while(|w| *w != "--").any(|w| tail_follows(w)),
        "get-content" | "gc" | "cat" | "type" => args.iter().any(|w| {
            let w = w.to_ascii_lowercase();
            w == "-wait" || (w.starts_with("-wait:") && w != "-wait:$false")
        }),
        shell @ ("bash" | "sh" | "zsh" | "powershell" | "pwsh") => {
            // Its own command: for bash the word after `-c` / `-lc`, for
            // PowerShell all that follows `-Command` / `-c`.
            let flag = |w: &str| {
                let w = w.to_ascii_lowercase();
                w == "-command" || (w.starts_with('-') && !w.starts_with("--") && w.ends_with('c') && w.len() <= 4)
            };
            let Some(at) = args.iter().position(|w| flag(w)) else { return false };
            let inner = if shell.starts_with("pw") || shell == "powershell" {
                args[at + 1..].join(" ")
            } else {
                args.get(at + 1).cloned().unwrap_or_default()
            };
            follows_unbounded(&inner)
        }
        _ => false,
    }
}

/// A `timeout` duration of zero (`0`, `0s`, `0.0m`): no deadline at all.
fn duration_is_zero(word: &str) -> bool {
    word.trim_end_matches(['s', 'm', 'h', 'd']).parse::<f64>().is_ok_and(|n| n == 0.0)
}

/// `tail`'s options that follow: `-f`, `-F`, `--follow[=…]`, or a cluster of
/// short ones holding either (`-fn0`, `-qF`); a count attached (`-n100`) ends
/// the cluster.
fn tail_follows(word: &str) -> bool {
    if word == "--follow" || word.starts_with("--follow=") {
        return true;
    }
    let Some(cluster) = word.strip_prefix('-').filter(|c| !c.starts_with('-')) else { return false };
    cluster.chars().take_while(|c| c.is_ascii_alphabetic()).any(|c| c == 'f' || c == 'F')
}

/// What a backslash escapes inside double quotes in bash. Outside quotes it
/// escapes any character but a letter or a digit, before which it stands for
/// itself: that keeps a Windows path (`C:\Tools\tail.exe`) whole.
const ESCAPED_IN_QUOTES: &str = "\"\\$`";

/// What a backtick escapes outside quotes, as PowerShell's escape. Before
/// anything else (a letter, mostly) it opens or closes a bash command
/// substitution, which is a command of its own.
const BACKTICK_ESCAPES: &str = "\"'`$|;&(){}<>#@ \t";

/// The simple commands of a command line, each as its words with quotes taken
/// off: split at `|`, `;`, `&`, newlines, parentheses and braces outside
/// quotes. Read for bash and PowerShell at once, as either may run it: a
/// backtick before a newline continues the line, as a backslash does, before a
/// special character escapes it (PowerShell), and else splits (bash). A `#` that starts a word comments
/// out the rest of its line; a heredoc's body (`<<EOF` … `EOF`) is skipped and
/// a PowerShell here-string (`@'` … `'@`) is one word.
fn simple_commands(line: &str) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut heredocs: Vec<String> = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    let end_word = |word: &mut String, in_word: &mut bool, words: &mut Vec<String>| {
        if *in_word {
            words.push(std::mem::take(word));
            *in_word = false;
        }
    };
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else if q == '"'
                && (c == '\\' && chars.get(i + 1).is_some_and(|n| ESCAPED_IN_QUOTES.contains(*n))
                    || c == '`' && chars.get(i + 1) == Some(&'"'))
            {
                // `\"` for bash, `` `" `` for PowerShell: a quote that does not end it.
                i += 1;
                word.push(chars[i]);
            } else {
                word.push(c);
            }
            i += 1;
            continue;
        }
        let continued = |at: usize| chars.get(at) == Some(&'\n') || (chars.get(at) == Some(&'\r') && chars.get(at + 1) == Some(&'\n'));
        match c {
            '\'' | '"' => {
                quote = Some(c);
                in_word = true;
            }
            // A line continued: the two lines are one.
            '\\' | '`' if continued(i + 1) => i += if chars[i + 1] == '\r' { 2 } else { 1 },
            '\\' if chars.get(i + 1).is_some_and(|n| !n.is_alphanumeric()) => {
                i += 1;
                word.push(chars[i]);
                in_word = true;
            }
            '`' if chars.get(i + 1).is_some_and(|n| BACKTICK_ESCAPES.contains(*n)) => {
                i += 1;
                word.push(chars[i]);
                in_word = true;
            }
            // A PowerShell here-string: `@'` or `@"` ending its line, to a
            // line that starts with the quote and `@`.
            '@' if !in_word && chars.get(i + 1).is_some_and(|q| *q == '\'' || *q == '"') && continued(i + 2) => {
                let q = chars[i + 1];
                i += 2;
                let mut body = String::new();
                while i < chars.len() && !(chars[i] == '\n' && chars.get(i + 1) == Some(&q) && chars.get(i + 2) == Some(&'@')) {
                    body.push(chars[i]);
                    i += 1;
                }
                word.push_str(&body);
                in_word = true;
                i += 3;
                continue;
            }
            '#' if !in_word => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            // A here-string (`<<<`) or a shift (`1 << 2`): no body to skip.
            '<' if chars.get(i + 1) == Some(&'<') && chars.get(i + 2) == Some(&'<') => {
                end_word(&mut word, &mut in_word, &mut words);
                i += 3;
                continue;
            }
            '<' if chars.get(i + 1) == Some(&'<') => {
                // `<<EOF`, `<<-EOF`, `<< 'EOF'`: the body goes from the next
                // line to one that is the delimiter alone. Bare digits are a
                // shift (`1 << 2`); quoted, any word is a delimiter.
                end_word(&mut word, &mut in_word, &mut words);
                i += 2;
                if chars.get(i) == Some(&'-') {
                    i += 1;
                }
                while chars.get(i) == Some(&' ') {
                    i += 1;
                }
                let mut delim = String::new();
                let mut quoted = false;
                while let Some(&d) = chars.get(i) {
                    if d.is_whitespace() || "|;&<>()".contains(d) {
                        break;
                    }
                    if d == '\'' || d == '"' {
                        quoted = true;
                    } else {
                        delim.push(d);
                    }
                    i += 1;
                }
                if !delim.is_empty() && (quoted || !delim.chars().all(|c| c.is_ascii_digit())) {
                    heredocs.push(delim);
                }
                continue;
            }
            '\n' => {
                end_word(&mut word, &mut in_word, &mut words);
                if !words.is_empty() {
                    out.push(std::mem::take(&mut words));
                }
                // Skip each heredoc's body, in order.
                for delim in std::mem::take(&mut heredocs) {
                    i += 1;
                    loop {
                        let start = i;
                        while i < chars.len() && chars[i] != '\n' {
                            i += 1;
                        }
                        let text: String = chars[start..i].iter().collect();
                        if text.trim() == delim || i >= chars.len() {
                            break;
                        }
                        i += 1;
                    }
                }
            }
            '|' | ';' | '&' | '(' | ')' | '{' | '}' | '`' => {
                end_word(&mut word, &mut in_word, &mut words);
                if !words.is_empty() {
                    out.push(std::mem::take(&mut words));
                }
            }
            c if c.is_whitespace() => end_word(&mut word, &mut in_word, &mut words),
            c => {
                word.push(c);
                in_word = true;
            }
        }
        i += 1;
    }
    end_word(&mut word, &mut in_word, &mut words);
    if !words.is_empty() {
        out.push(words);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_follow_with_no_deadline_is_refused() {
        for cmd in [
            "tail -f run.log",
            "tail -F a.log | grep --line-buffered DONE",
            "tail -n +1 -f run.log | grep -m1 DONE",
            "tail -fn0 x.log",
            "tail -qF x.log",
            "tail --follow=name x.log",
            "cd build && tail -f out.log",
            "(tail -f y.log &)",
            "/usr/bin/tail -f y.log",
            "Get-Content run.log -Wait",
            "gc .\\x.log -Tail 5 -Wait | Select-String DONE",
            "bash -c \"tail -f x.log\"",
            "powershell -NoProfile -Command \"Get-Content x.log -Wait\"",
            "ls\ntail -f x.log",
            "timeout 60 ls; tail -f x.log",
            "nohup stdbuf -oL tail -f x.log > out &",
            "LC_ALL=C tail -f x.log",
            "tail \\\n  -f x.log",
            "$lines = Get-Content x.log -Wait",
            "if test -f log; then tail -f log; fi",
            "while true; do tail -f log; done",
            "powershell -Command Get-Content log -Wait",
            "Get-Content log -Wait:$true",
            "Get-Content log `\n  -Wait",
            "timeout 0 tail -f log",
            "timeout -s KILL 0s tail -f log",
            "cat <<< hello\ntail -f log",
            "((x = 1 << 2))\ntail -f log",
            "& \"C:\\Tools\\tail.exe\" -f log",
            "C:\\Tools\\tail.exe -F log",
            "echo `tail -f log`",
            "tail \\-f log",
        ] {
            assert!(follows_unbounded(cmd), "{cmd}");
        }
    }

    #[test]
    fn a_follow_with_a_deadline_or_no_follow_goes_ahead() {
        for cmd in [
            "timeout 1800 tail -n +1 -f run.log | grep --line-buffered -E 'STEP|FAIL'",
            "timeout 60s tail -F x.log",
            "timeout --signal=KILL 60 tail -f x.log",
            "timeout 60 bash -c \"tail -f x.log\"",
            "timeout 60 powershell -c \"Get-Content x.log -Wait\"",
            "tail -n 50 x.log",
            "tail -n100 x.log",
            "tail x.log",
            "Get-Content x.log -Tail 10",
            "git commit -m \"Refuse tail -f without a timeout\"",
            "echo 'tail -f run.log'",
            "grep -n -- '-f' tail.txt",
            "grep tail -f patterns.txt",
            "nohup timeout 600 tail -f x.log &",
            "cat > watch.sh <<'EOF'\ntail -f log\nEOF\nbash -n watch.sh",
            "# tail -f run.log\nls",
            "Set-Content watch.sh @'\n# don't wait\ntail -f log\n'@",
            "Set-Content watch.ps1 @\"\r\nGet-Content log -Wait\r\n\"@\r\n& .\\check.ps1",
            "Write-Output `; tail -f log",
            "tail -- -file.log",
            "Get-Content log -Wait:$false",
            "timeout -s KILL 60 tail -f log",
            "timeout -k 5 60 tail -f log",
            "cat <<'123'\ntail -f log\n123",
            "Write-Output \"example: `\"; tail -f log\"",
            "",
        ] {
            assert!(!follows_unbounded(cmd), "{cmd}");
        }
    }

    #[test]
    fn the_decision_is_a_deny_for_both_clis() {
        let call = |tool: &str, command: &str| {
            decision(
                &serde_json::json!({ "hook_event_name": "PreToolUse", "tool_name": tool, "tool_input": { "command": command } })
                    .to_string(),
            )
        };
        let deny = call("Bash", "tail -f x.log").expect("refused");
        let v: serde_json::Value = serde_json::from_str(&deny).unwrap();
        assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        let reason = v["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap();
        assert!(reason.contains("timeout 1800") && reason.contains("Start-Sleep"), "{reason}");
        // What it suggests, in either shell, it lets through.
        for suggested in reason.split('`').filter(|s| s.starts_with("timeout ") || s.starts_with("$end")) {
            assert!(!follows_unbounded(suggested), "{suggested}");
        }
        assert!(call("PowerShell", "Get-Content x.log -Wait").is_some());
        assert!(call("Monitor", "tail -f x.log | grep DONE").is_some());
        assert!(call("Bash", "timeout 600 tail -f x.log").is_none());
        assert!(call("Read", "tail -f x.log").is_none(), "not a shell");
        assert!(decision("not json").is_none());
    }
}
