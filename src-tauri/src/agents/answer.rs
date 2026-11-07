//! `luna_answer`: a session's question or permission prompt answered with the
//! keys its CLI takes, by option number, label or words.

use crate::agents::ask::{ask_of, live, resolve_option, state_of, Ask};
use crate::agents::delivery::{screen_of, type_in, TypingInto, ENTER_GAP, SEND_TYPING_MS, TYPING_WAIT};
use crate::agents::owed::{owe_from, seen};
use crate::agents::privilege::{answerer, approval_needs, command_reach_now, plain_text, Privilege, Reach};
use crate::agents::read::transcript_end;
use crate::agents::registry::{authorize_send, reg};
use crate::provider::Provider;
use crate::throttle::now_ms;
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Answered {
    pub ok: bool,
    /// Where it stands now (see `state_of`): a dialog that is still up, or
    /// the next question of several, shows here and in `screen`.
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask: Option<Ask>,
    /// The `afterTurn` for the `wait` that collects the reply.
    pub turns_ended: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen: Option<String>,
}

/// One press for a terminal: text as typed, or a key's escape sequence.
#[derive(Debug, PartialEq)]
pub(super) enum Key {
    Text(String),
    Seq(&'static str),
}

/// `{enter}`, `{esc}`, `{down}` … between literal text, as `luna_answer`'s
/// `keys` take them.
fn parse_keys(spec: &str) -> Result<Vec<Key>, String> {
    let mut out = Vec::new();
    let mut rest = spec;
    while !rest.is_empty() {
        let Some(open) = rest.find('{') else {
            out.push(Key::Text(rest.to_string()));
            break;
        };
        if open > 0 {
            out.push(Key::Text(rest[..open].to_string()));
        }
        let close = rest[open..].find('}').ok_or("a { without its }")? + open;
        let seq = match rest[open + 1..close].trim().to_lowercase().as_str() {
            "enter" => "\r",
            "esc" => "\x1b",
            "tab" => "\t",
            "shift+tab" => "\x1b[Z",
            "up" => "\x1b[A",
            "down" => "\x1b[B",
            "right" => "\x1b[C",
            "left" => "\x1b[D",
            "shift+left" => "\x1b[1;2D",
            "shift+right" => "\x1b[1;2C",
            "space" => " ",
            "backspace" => "\x7f",
            "ctrl+c" => "\x03",
            other => return Err(format!("unknown key {{{other}}}")),
        };
        out.push(Key::Seq(seq));
        rest = &rest[close + 1..];
    }
    Ok(out)
}

/// The presses that answer `ask` in a CLI's terminal: an option is reached
/// with the arrows from the first one and taken with Enter; words of its own
/// go in the free-text row that follows the options; a permission is
/// Enter on its first choice (yes) or Esc (no, with the note typed after as
/// what to do instead).
fn presses_for(provider: Provider, ask: &Ask, answers: &[String], allow: Option<bool>, note: Option<&str>) -> Result<Vec<Key>, String> {
    let mut keys = Vec::new();
    if ask.kind == "permission" {
        match allow {
            Some(true) => keys.push(match provider {
                Provider::Claude => Key::Seq("\r"),
                Provider::Codex => Key::Text("y".into()),
            }),
            Some(false) => {
                keys.push(Key::Seq("\x1b"));
                if let Some(n) = note.map(str::trim).filter(|n| !n.is_empty()) {
                    keys.push(Key::Text(n.to_string()));
                    keys.push(Key::Seq("\r"));
                }
            }
            None => return Err("this is a permission: answer with allow true or false".into()),
        }
        return Ok(keys);
    }
    if answers.len() != ask.questions.len() {
        return Err(format!("{} question(s), {} answer(s): give one answer per question", ask.questions.len(), answers.len()));
    }
    for (q, a) in ask.questions.iter().zip(answers) {
        let picked = resolve_option(q, a);
        if let Some(at) = q.list_at {
            let i =
                q.options.iter().position(|o| *o == picked).ok_or_else(|| {
                    format!("pick one of the options of \"{}\", by number or label; it takes no words of one's own", q.text)
                })?;
            let (key, n) = if i >= at { ("\x1b[B", i - at) } else { ("\x1b[A", at - i) };
            keys.extend(std::iter::repeat_with(|| Key::Seq(key)).take(n));
            keys.push(Key::Seq("\r"));
            continue;
        }
        match q.options.iter().position(|o| *o == picked) {
            Some(i) => keys.extend(std::iter::repeat_with(|| Key::Seq("\x1b[B")).take(i)),
            None => {
                keys.extend(std::iter::repeat_with(|| Key::Seq("\x1b[B")).take(q.options.len()));
                keys.push(Key::Text(picked));
            }
        }
        keys.push(Key::Seq("\r"));
    }
    // Several questions end on a review of the answers, to submit.
    if ask.questions.len() > 1 && provider == Provider::Claude {
        keys.push(Key::Seq("\r"));
    }
    Ok(keys)
}

/// Writes presses one at a time, far enough apart that the CLI takes each
/// as a key and not a burst of paste (see ENTER_GAP).
pub(super) fn press(pty: &crate::pty::PtyManager, id: &str, keys: &[Key]) -> Result<(), String> {
    for k in keys {
        match k {
            Key::Text(t) => {
                pty.write(id, t.as_bytes())?;
                std::thread::sleep(ENTER_GAP);
            }
            Key::Seq(s) => {
                pty.write(id, s.as_bytes())?;
                std::thread::sleep(KEY_GAP);
            }
        }
    }
    Ok(())
}

/// How long an interrupted turn gets to read as over before the text goes in.
pub(super) const INTERRUPT_SETTLE: std::time::Duration = std::time::Duration::from_secs(3);

/// Between two keys pressed for an answer.
const KEY_GAP: std::time::Duration = std::time::Duration::from_millis(150);

/// How long after an answer its effect is looked at.
const ANSWER_SETTLE: std::time::Duration = std::time::Duration::from_millis(1500);

/// `luna_answer`: answers what the session waits on — a question or a
/// permission (`ask_of`), or with `keys`, any dialog on its screen.
/// Under a drain (power/) only a structured answer goes through, to what a session
/// holds for one now, named where it has a name. Raw keys are refused outright: a
/// permission's dialog takes `{enter}` and the rest is a new prompt. So is a note, which
/// a refused permission's terminal takes as one, and a line break in an answer's own
/// words, which would send what follows. A dialog Luna cannot read waits for a person.
fn drain_lets_answer(
    id: &str,
    ask: Option<&Ask>,
    ask_id: Option<&str>,
    answers: &[String],
    note: Option<&str>,
    keys: Option<&str>,
) -> Result<(), String> {
    if keys.is_some() {
        return Err("Luna is draining: keys are refused; answer with answers or allow".into());
    }
    if note.is_some_and(|n| !n.trim().is_empty()) {
        return Err("Luna is draining: a note is refused; answer with allow alone".into());
    }
    if answers.iter().any(|a| a.contains(['\r', '\n'])) {
        return Err("Luna is draining: an answer is one line".into());
    }
    let Some(ask) = ask else {
        return Err(format!(
            "Luna is draining, and session {id} has no question or permission pending that Luna can read, so nothing goes to it"
        ));
    };
    if ask.id.is_some() && ask.id.as_deref() != ask_id {
        return Err(format!("Luna is draining: name the ask you answer, askId {:?}", ask.id.as_deref().unwrap_or("")));
    }
    Ok(())
}

/// What `luna_answer` may do to a session that is not one of the caller's
/// helpers: answer a question its model asked. Approving or refusing a
/// permission, raw keys, and a list a CLI holds its own screen on (Codex's
/// approvals and hook trust read as one, `codex_list_on`) are its owner's and
/// the user's to settle — they decide what the session may do.
///
/// On its own helper, a caller says yes to a permission (allow true) only where
/// it would itself run that tool unasked (`approval_needs`), and never from
/// read-only or asking modes: a helper is no back door past the person its
/// caller would have asked. Raw keys and a CLI's own lists take full access.
/// `approver`: the
/// caller's privilege, None where the user lets helpers go above their caller.
pub(super) fn answer_reach(
    own: bool,
    approver: Option<Privilege>,
    provider: Provider,
    ask: Option<&Ask>,
    allow: Option<bool>,
    keys: Option<&str>,
) -> Result<(), String> {
    if own {
        let Some(has) = approver else { return Ok(()) };
        let full = Privilege { reach: Reach::Full, net: true };
        // Keys reach whatever the terminal shows, detected or not, and the
        // CLI's own switches with it (shift+tab changes Claude Code's mode):
        // a caller at full access gives up nothing by them.
        if keys.is_some() && !has.covers(full) {
            return Err(format!(
                "raw keys reach any dialog of the CLI and its mode switches, so they are for callers at full access; \
                 you run at {}. Answer with answers or allow",
                has.label()
            ));
        }
        let Some(a) = ask else { return Ok(()) };
        // A list the CLI holds its own screen on (hook trust, its approvals)
        // decides what runs, in ways Luna does not weigh: any answer is a yes
        // at full access.
        if a.questions.iter().any(|q| q.list_at.is_some()) {
            if has.covers(full) {
                return Ok(());
            }
            return Err(format!(
                "\"{}\" is the CLI's own and may widen what runs: it goes to the user in Luna's window, unless you \
                 run at full access (you run at {})",
                a.questions.first().map_or("", |q| q.text.as_str()),
                has.label()
            ));
        }
        if a.kind != "permission" || allow != Some(true) {
            return Ok(());
        }
        let needs = approval_needs(provider, a);
        if has.reach >= Reach::Edits && has.covers(needs) {
            return Ok(());
        }
        return Err(format!(
            "approving {} would run it at {}, above what you run unasked ({}): it goes to the user in Luna's window. \
             You may refuse it, with allow false",
            a.tool.as_deref().unwrap_or("this"),
            needs.label(),
            has.label()
        ));
    }
    let what = if keys.is_some() {
        "keys"
    } else if allow.is_some() || ask.is_some_and(|a| a.kind == "permission") {
        "a permission"
    } else if ask.is_some_and(|a| a.questions.iter().any(|q| q.list_at.is_some())) {
        "a dialog of the CLI's own"
    } else {
        return Ok(());
    };
    Err(format!(
        "{what} only to sessions you spawned, directly or through a helper; this one is not yours. Its own chat or \
         the user answers it; you may answer a question its model asked"
    ))
}

/// Answers and a note go into a terminal's dialog as keys (`presses_for`): a
/// line break there submits the field, and what follows lands in the composer
/// as a prompt of its own (`answer\r/permissions …`); a tab moves on.
fn typed_answers(answers: &[String], note: Option<&str>) -> Result<(), String> {
    if answers.iter().map(String::as_str).chain(note).any(|t| t.contains(['\r', '\n', '\t'])) {
        return Err("an answer or note typed into a terminal is one line, without tabs".into());
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn answer(
    caller: &str,
    id: &str,
    ask_id: Option<&str>,
    answers: &[String],
    allow: Option<bool>,
    note: Option<&str>,
    keys: Option<&str>,
    draining: bool,
) -> Result<Answered, String> {
    let known = authorize_send(caller, id)?;
    // Typed in as keys: what is not plain text would be keys of its own.
    for t in answers.iter().map(String::as_str).chain(note) {
        plain_text(t)?;
    }
    let background = crate::background::has(id);
    if !background {
        typed_answers(answers, note)?;
        // A refusal's note is typed after the Esc that closes the dialog, into
        // the composer, and Enter sends it; an answer of one's own words lands
        // there too if its dialog has gone by the time it is typed. Either is
        // held to what may be typed there (`command_reach`).
        for t in answers.iter().map(String::as_str).chain(note) {
            command_reach_now(caller, id, t)?;
        }
    }
    let (own, approver) = answerer(caller, id);
    answer_reach(own, approver, known.provider, None, allow, keys)?;
    let pty = crate::pty::manager();
    if !live(pty, id) {
        return Err(format!("session {id} is not running"));
    }
    // Keys in the middle of a notice's paste would land in it: its turn first,
    // and what is to be answered as it stands after that. A background helper
    // has no terminal.
    let terminal = if crate::background::has(id) {
        None
    } else {
        Some(
            TypingInto::wait_for(id, TYPING_WAIT)
                .ok_or_else(|| format!("Luna is still typing into session {id}; try again in a little while"))?,
        )
    };
    let baseline = crate::activity::turn_state(id).map(|t| t.turns_ended).unwrap_or(0);
    let ask = ask_of(id);
    // A background helper's answer is checked against the very ask it lands
    // on, as it is recorded (`background::answer`): the first held, read here,
    // need not be the one an askId names.
    if !background {
        answer_reach(own, approver, known.provider, ask.as_ref(), allow, keys)?;
    }
    if draining {
        drain_lets_answer(id, ask.as_ref(), ask_id, answers, note, keys)?;
    }
    // What the session says on the answer starts right away, long before
    // the call returns.
    let cursor = transcript_end(id);
    if crate::background::has(id) {
        if keys.is_some() {
            return Err(
                "a background helper has no terminal: answer with answers or allow, or stop its turn with luna_send interrupt: true".into(),
            );
        }
        // Checked against the ask it lands on, as it is recorded.
        let may = |a: &Ask| answer_reach(own, approver, known.provider, Some(a), allow, keys);
        crate::background::answer(id, ask_id, answers, allow, note, may)?;
    } else {
        // A person's keys between Luna's would land in the dialog or the answer.
        let hold = pty.hold_keys(id).ok_or("no such session")?;
        if pty.has_draft(id) || pty.typed_ago_ms(id).is_some_and(|ms| ms < SEND_TYPING_MS) {
            return Err(format!("the user is typing in session {id}; try again in a little while"));
        }
        match (keys, &ask) {
            (Some(k), _) => press(pty, id, &parse_keys(k)?)?,
            // An async question holds no dialog: the answer is a message.
            (None, Some(a)) if !a.blocking => {
                let text: Vec<String> =
                    a.questions.iter().zip(answers).map(|(q, ans)| format!("\"{}\": {}", q.text, resolve_option(q, ans))).collect();
                if text.is_empty() {
                    return Err("give one answer per question".into());
                }
                let before = crate::activity::turn_state(id);
                type_in(pty, id, &format!("Answers to your question: {}", text.join("; ")), before.map(|t| t.turn), baseline, hold)?;
            }
            (None, Some(a)) => press(pty, id, &presses_for(known.provider, a, answers, allow, note)?)?,
            (None, None) => {
                return Err(format!(
                    "session {id} is not on a question or permission Luna can read; luna_read with screen: true shows its \
                     terminal, and keys answers any dialog"
                ))
            }
        }
        reg().lock().unwrap_or_else(|e| e.into_inner()).answered.insert((caller.to_string(), id.to_string()), now_ms());
    }
    drop(terminal);
    std::thread::sleep(ANSWER_SETTLE);
    seen(caller, id, baseline);
    owe_from(caller, id, baseline, cursor);
    let (state, ask) = state_of(id);
    let screen = (!crate::background::has(id)).then(|| screen_of(pty, id)).flatten();
    Ok(Answered { ok: true, state, ask, turns_ended: baseline, screen })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::ask::{codex_command_on, codex_list_on, resolve_option, Ask, AskQuestion};
    use crate::agents::privilege::{Privilege, Reach};
    use crate::agents::registry::register;
    use crate::agents::teardown::forget;

    #[test]
    fn keys_read_as_presses() {
        assert_eq!(parse_keys("2{enter}").unwrap(), [Key::Text("2".into()), Key::Seq("\r")]);
        assert_eq!(parse_keys("{Down}{down}{ENTER}").unwrap(), [Key::Seq("\x1b[B"), Key::Seq("\x1b[B"), Key::Seq("\r")]);
        assert!(parse_keys("{f13}").is_err());
        assert!(parse_keys("{enter").is_err());
    }

    #[test]
    fn an_answer_picks_by_number_label_or_words() {
        let q = AskQuestion { text: "Which?".into(), options: vec!["Red".into(), "Blue".into()], multi: false, list_at: None };
        assert_eq!(resolve_option(&q, "2"), "Blue");
        assert_eq!(resolve_option(&q, "red"), "Red");
        assert_eq!(resolve_option(&q, "9"), "9", "out of range is words");
        assert_eq!(resolve_option(&q, " green "), "green");
        let multi = AskQuestion { multi: true, ..q.clone() };
        assert_eq!(resolve_option(&multi, "1, 2"), "Red, Blue");

        let ask = Ask::question(vec![q], true);
        let keys = presses_for(Provider::Claude, &ask, &["2".into()], None, None).unwrap();
        assert_eq!(keys, [Key::Seq("\x1b[B"), Key::Seq("\r")]);
        let own = presses_for(Provider::Claude, &ask, &["green".into()], None, None).unwrap();
        assert_eq!(own, [Key::Seq("\x1b[B"), Key::Seq("\x1b[B"), Key::Text("green".into()), Key::Seq("\r")]);
        assert!(presses_for(Provider::Claude, &ask, &[], None, None).is_err());
        let perm = Ask::permission(Some("Bash".into()), Some("ls".into()));
        assert_eq!(presses_for(Provider::Codex, &perm, &[], Some(true), None).unwrap(), [Key::Text("y".into())]);
        assert_eq!(
            presses_for(Provider::Claude, &perm, &[], Some(false), Some("use dir")).unwrap(),
            [Key::Seq("\x1b"), Key::Text("use dir".into()), Key::Seq("\r")]
        );
        assert_eq!(perm.line(), "asks for permission to run Bash: ls");
        let codex =
            "  Would you like to run the following command?\n\n  $ Set-Content -LiteralPath made.txt -Value hi\n\n› 1. Yes, proceed (y)";
        assert_eq!(codex_command_on(codex).as_deref(), Some("Set-Content -LiteralPath made.txt -Value hi"));
    }

    #[test]
    fn a_codex_list_on_screen_is_a_question_answered_by_its_own_options() {
        let screen = "\n  Hooks need review\n  5 hooks are new or changed.\n  Hooks can run outside the sandbox after you trust them.\n\n\n\
                      › 1. Review hooks\n  2. Trust all and continue\n  3. Continue without trusting (hooks won't run)\n\n  enter confirm · esc skip\n";
        let ask = codex_list_on(screen).unwrap();
        let q = &ask.questions[0];
        assert_eq!(q.text, "Hooks need review 5 hooks are new or changed. Hooks can run outside the sandbox after you trust them.");
        assert_eq!(q.options, ["Review hooks", "Trust all and continue", "Continue without trusting (hooks won't run)"]);
        assert!(ask.blocking && ask.kind == "question");
        let keys = presses_for(Provider::Codex, &ask, &["3".into()], None, None).unwrap();
        assert_eq!(keys, [Key::Seq("\x1b[B"), Key::Seq("\x1b[B"), Key::Seq("\r")]);
        assert!(presses_for(Provider::Codex, &ask, &["trust them all".into()], None, None).is_err(), "no words of one's own");

        // From where the cursor is, not from the top.
        let moved = screen.replace("› 1.", "  1.").replace("  2. Trust", "› 2. Trust");
        let ask = codex_list_on(&moved).unwrap();
        let keys = presses_for(Provider::Codex, &ask, &["Review hooks".into()], None, None).unwrap();
        assert_eq!(keys, [Key::Seq("\x1b[A"), Key::Seq("\r")]);

        // A numbered list in the conversation, with the composer below it, is none.
        let chat = "• Steps:\n  1. Build\n  2. Test\n\n› Ask Codex to do anything\n\n  ? for shortcuts";
        assert!(codex_list_on(chat).is_none());
        let selecting = chat.replace("? for shortcuts", "ctrl+c copy · enter copy & follow · esc clear");
        assert!(codex_list_on(&selecting).is_none());
    }

    #[test]
    fn a_drain_lets_only_an_identified_ask_be_answered() {
        let ask = |id: Option<&str>, blocking: bool| Ask {
            id: id.map(str::to_owned),
            kind: "question",
            blocking,
            questions: Vec::new(),
            tool: None,
            detail: None,
        };
        let plain: &[String] = &[];
        let refused = |r: Result<(), String>| r.unwrap_err();
        // Nothing pending.
        assert!(refused(drain_lets_answer("s", None, None, plain, None, None)).contains("no question or permission pending"));
        // Keys never, whatever is held: past a dialog they are a prompt.
        let keys = Some("{enter}start another task{enter}");
        for held in [None, Some(ask(None, true)), Some(ask(None, false)), Some(ask(Some("a1"), true))] {
            assert!(refused(drain_lets_answer("s", held.as_ref(), Some("a1"), plain, None, keys)).contains("keys are refused"));
        }
        // A structured answer goes to what is held.
        assert_eq!(drain_lets_answer("s", Some(&ask(None, true)), None, plain, None, None), Ok(()));
        assert_eq!(drain_lets_answer("s", Some(&ask(None, false)), None, &["yes".to_string()], None, None), Ok(()));
        // Not with what a terminal takes as a prompt in it.
        assert!(refused(drain_lets_answer("s", Some(&ask(None, true)), None, plain, Some("do this instead"), None)).contains("a note"));
        assert_eq!(drain_lets_answer("s", Some(&ask(None, true)), None, plain, Some("  "), None), Ok(()));
        assert!(
            refused(drain_lets_answer("s", Some(&ask(None, true)), None, &["a\rnew prompt".to_string()], None, None)).contains("one line")
        );
        // A background helper's has an id, and it is to be named.
        assert!(refused(drain_lets_answer("s", Some(&ask(Some("a1"), true)), None, plain, None, None)).contains("a1"));
        assert!(refused(drain_lets_answer("s", Some(&ask(Some("a1"), true)), Some("a0"), plain, None, None)).contains("a1"));
        assert_eq!(drain_lets_answer("s", Some(&ask(Some("a1"), true)), Some("a1"), plain, None, None), Ok(()));
    }

    #[test]
    fn allow_and_keys_reach_only_the_callers_own_helpers() {
        let ask = |kind: &'static str, list_at: Option<usize>| Ask {
            id: None,
            kind,
            blocking: true,
            questions: vec![AskQuestion { text: "Which?".into(), options: vec!["A".into(), "B".into()], multi: false, list_at }],
            tool: None,
            detail: None,
        };
        let (question, permission, dialog) = (ask("question", None), ask("permission", None), ask("question", Some(0)));
        let full = Some(Privilege { reach: Reach::Full, net: true });
        for own in [true, false] {
            // Answers to a question its model asked: anyone's.
            assert_eq!(answer_reach(own, full, Provider::Claude, Some(&question), None, None), Ok(()));
            assert_eq!(answer_reach(own, full, Provider::Claude, None, None, None), Ok(()), "nothing known pending yet");
            for (held, allow, keys) in [
                (Some(&permission), Some(true), None),
                (Some(&permission), Some(false), None),
                (Some(&permission), None, None),
                (None, Some(true), None),
                (None, None, Some("{enter}")),
                (Some(&question), None, Some("{down}{enter}")),
                (Some(&dialog), None, None),
            ] {
                let r = answer_reach(own, full, Provider::Codex, held, allow, keys);
                assert_eq!(r.is_ok(), own, "{:?} allow {allow:?} keys {keys:?}: {r:?}", held.map(|a| a.kind));
                if let Err(e) = r {
                    assert!(e.contains("only to sessions you spawned"), "{e}");
                }
            }
        }
    }

    #[test]
    fn an_answer_typed_as_keys_is_one_line() {
        let one = |a: &str| vec![a.to_string()];
        assert_eq!(typed_answers(&one("Blue, and a word of my own"), Some("not now")), Ok(()));
        for bad in ["answer\r/permissions :workspace", "a\nb", "a\tb"] {
            assert!(typed_answers(&one(bad), None).unwrap_err().contains("one line"), "{bad:?}");
            assert!(typed_answers(&one("ok"), Some(bad)).is_err(), "a note too: {bad:?}");
        }
    }

    #[test]
    fn a_refusals_note_typed_into_a_terminal_is_held_to_what_may_be_typed_there() {
        let mode = |m: &str| serde_json::json!({ "permissionMode": m });
        register("rn-asks", Provider::Claude, "C:/p", "C:/acc/one", None, true, mode("default"));
        register("rn-full", Provider::Claude, "C:/p", "C:/acc/one", None, true, mode("bypassPermissions"));
        register("rn-kid", Provider::Claude, "C:/p", "C:/acc/one", Some("rn-asks"), false, mode("default"));
        register("rn-kid2", Provider::Codex, "C:/p", "C:/acc/one", Some("rn-full"), false, mode("default"));
        let refuse = |caller: &str, id: &str, note: &str| {
            answer(caller, id, None, &[], Some(false), Some(note), None, false).err().unwrap_or_default()
        };
        // After the Esc, the note is a prompt in the composer: a command there
        // is refused before any key goes in.
        for note in ["! rm -rf build", "  /permissions :workspace"] {
            assert!(refuse("rn-asks", "rn-kid", note).contains("not a prompt"), "{note:?}");
        }
        // So is an answer in one's own words.
        let e = answer("rn-asks", "rn-kid", None, &["!whoami".to_string()], None, None, None, false).err().unwrap();
        assert!(e.contains("not a prompt"), "{e}");
        // Plain words pass that check (and stop at the session not running).
        assert!(refuse("rn-asks", "rn-kid", "not that file").contains("is not running"));
        // A caller at full access, into its own helper.
        assert!(refuse("rn-full", "rn-kid2", "! make test").contains("is not running"));
        for id in ["rn-kid", "rn-kid2", "rn-asks", "rn-full"] {
            forget(id);
        }
    }
}
