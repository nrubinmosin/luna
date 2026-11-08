//! `luna_answer`: what a session waits on, answered. A helper's question or
//! permission goes to its record (background/ask.rs); a question a chat's model
//! asked goes in as the keys its CLI takes, by option number, label or words.

use crate::agents::ask::{ask_of, live, resolve_option, state_of, Ask};
use crate::agents::delivery::{as_sent, screen_of, type_in, TypingInto, ENTER_GAP, SEND_TYPING_MS, TYPING_WAIT};
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

/// The presses that answer a question in a CLI's terminal: an option is reached
/// with the arrows from the first one and taken with Enter; words of its own go
/// in the free-text row that follows the options. A permission in a terminal is
/// never an agent's to answer (`answer_reach`): the chats there are the person's.
fn presses_for(provider: Provider, ask: &Ask, answers: &[String]) -> Result<Vec<Key>, String> {
    if ask.kind == "permission" {
        return Err("a permission in a chat of the person's is theirs to answer".into());
    }
    let mut keys = Vec::new();
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
fn press(pty: &crate::pty::PtyManager, id: &str, keys: &[Key]) -> Result<(), String> {
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

/// Between two keys pressed for an answer.
const KEY_GAP: std::time::Duration = std::time::Duration::from_millis(150);

/// How long after an answer its effect is looked at.
const ANSWER_SETTLE: std::time::Duration = std::time::Duration::from_millis(1500);

/// `luna_answer`: answers what the session waits on — a question or a
/// permission (`ask_of`).
/// Under a drain (power/) only a structured answer goes through, to what a session
/// holds for one now, named where it has a name. A note is refused, which a refused
/// permission's terminal would take as a prompt, and so is a line break in an
/// answer's own words, which would send what follows. A dialog Luna cannot read
/// waits for a person.
fn drain_lets_answer(id: &str, ask: Option<&Ask>, ask_id: Option<&str>, answers: &[String], note: Option<&str>) -> Result<(), String> {
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
/// permission, and a list a CLI holds its own screen on (Codex's approvals and
/// hook trust read as one, `codex_list_on`), are its owner's and the user's to
/// settle — they decide what the session may do.
///
/// On its own helper, a caller says yes to a permission (allow true) only where
/// it would itself run that tool unasked (`approval_needs`), and never from
/// read-only or asking modes: a helper is no back door past the person its
/// caller would have asked. A CLI's own lists take full access.
/// `approver`: the caller's privilege, None where the user lets helpers go above
/// their caller.
pub(super) fn answer_reach(
    own: bool,
    approver: Option<Privilege>,
    provider: Provider,
    ask: Option<&Ask>,
    allow: Option<bool>,
) -> Result<(), String> {
    if own {
        let Some(has) = approver else { return Ok(()) };
        let full = Privilege { reach: Reach::Full, net: true };
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
    let what = if allow.is_some() || ask.is_some_and(|a| a.kind == "permission") {
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

/// Answers go into a terminal's dialog as keys (`presses_for`): a line break
/// there submits the field, and what follows lands in the composer as a prompt
/// of its own (`answer\r/permissions …`); a tab moves on.
fn typed_answers(answers: &[String], note: Option<&str>) -> Result<(), String> {
    if answers.iter().map(String::as_str).chain(note).any(|t| t.contains(['\r', '\n', '\t'])) {
        return Err("an answer or note typed into a terminal is one line, without tabs".into());
    }
    Ok(())
}

/// The message that answers a question with no dialog (Claude Code's async
/// `AskUserQuestion`): each answer beside its question, by number, label or
/// the caller's words. One answer per question exactly, as for a dialog
/// (`presses_for`): a missing one would leave a question unanswered with the
/// rest sent as if complete, an extra one would be dropped unsaid.
fn async_answers(ask: &Ask, answers: &[String]) -> Result<String, String> {
    if ask.questions.is_empty() || answers.len() != ask.questions.len() {
        return Err(format!("{} question(s), {} answer(s): give one answer per question", ask.questions.len(), answers.len()));
    }
    let text: Vec<String> =
        ask.questions.iter().zip(answers).map(|(q, ans)| format!("\"{}\": {}", q.text, resolve_option(q, ans))).collect();
    Ok(format!("Answers to your question: {}", text.join("; ")))
}

/// `async_answers` as typed into `id`: a message like any other the caller sends
/// there, opened with whose it is unless `id` is the caller's own (`as_sent`).
fn answer_message(caller: &str, id: &str, ask: &Ask, answers: &[String]) -> Result<String, String> {
    Ok(as_sent(caller, id, &async_answers(ask, answers)?))
}

pub fn answer(
    caller: &str,
    id: &str,
    ask_id: Option<&str>,
    answers: &[String],
    allow: Option<bool>,
    note: Option<&str>,
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
        // An answer of one's own words lands in the composer if its dialog has
        // gone by the time it is typed: held to what may be typed there
        // (`command_reach`).
        for t in answers.iter().map(String::as_str).chain(note) {
            command_reach_now(caller, id, t)?;
        }
    }
    let (own, approver) = answerer(caller, id);
    answer_reach(own, approver, known.provider, None, allow)?;
    let pty = crate::pty::manager();
    if !live(pty, id) {
        return Err(format!("session {id} is not running"));
    }
    // Keys in the middle of a notice's paste would land in it: its turn first,
    // and what is to be answered as it stands after that. A background helper
    // has no terminal.
    let terminal = if background {
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
        answer_reach(own, approver, known.provider, ask.as_ref(), allow)?;
    }
    if draining {
        drain_lets_answer(id, ask.as_ref(), ask_id, answers, note)?;
    }
    // What the session says on the answer starts right away, long before
    // the call returns.
    let cursor = transcript_end(id);
    if background {
        // Checked against the ask it lands on, as it is recorded.
        let may = |a: &Ask| answer_reach(own, approver, known.provider, Some(a), allow);
        crate::background::answer(id, ask_id, answers, allow, note, may)?;
    } else {
        // A person's keys between Luna's would land in the dialog or the answer.
        let hold = pty.hold_keys(id).ok_or("no such session")?;
        if pty.has_draft(id) || pty.typed_ago_ms(id).is_some_and(|ms| ms < SEND_TYPING_MS) {
            return Err(format!("the user is typing in session {id}; try again in a little while"));
        }
        match &ask {
            // An async question holds no dialog: the answer is a message.
            Some(a) if !a.blocking => {
                let text = answer_message(caller, id, a, answers)?;
                let before = crate::activity::turn_state(id);
                type_in(pty, id, &text, before.map(|t| t.turn), baseline, hold)?;
            }
            Some(a) => press(pty, id, &presses_for(known.provider, a, answers)?)?,
            None => return Err(format!("session {id} is not on a question Luna can read; luna_read with screen: true shows its terminal")),
        }
        reg().lock().unwrap_or_else(|e| e.into_inner()).answered.insert((caller.to_string(), id.to_string()), now_ms());
    }
    drop(terminal);
    std::thread::sleep(ANSWER_SETTLE);
    seen(caller, id, baseline);
    owe_from(caller, id, baseline, cursor);
    let (state, ask) = state_of(id);
    let screen = (!background).then(|| screen_of(pty, id)).flatten();
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
    fn an_async_question_takes_one_answer_per_question() {
        let q = |text: &str| AskQuestion { text: text.into(), options: vec!["Red".into(), "Blue".into()], multi: false, list_at: None };
        let ask = Ask::question(vec![q("Which?"), q("And then?")], false);
        let both = async_answers(&ask, &["1".into(), "blue".into()]).unwrap();
        assert_eq!(both, "Answers to your question: \"Which?\": Red; \"And then?\": Blue");
        for given in [vec![], vec!["1".to_string()], vec!["1".into(), "2".into(), "green".into()]] {
            let e = async_answers(&ask, &given).unwrap_err();
            assert!(e.starts_with(&format!("2 question(s), {} answer(s)", given.len())), "{e}");
        }
        assert!(async_answers(&Ask::question(Vec::new(), false), &[]).is_err(), "nothing to answer");
    }

    #[test]
    fn an_answer_typed_into_a_session_not_ones_own_says_whose_it_is() {
        let (account, none) = ("C:\\acc\\anthropic\\one", serde_json::json!({}));
        register("am-me", Provider::Claude, "C:\\p", account, None, true, none.clone());
        register("am-kid", Provider::Claude, "C:\\p", account, Some("am-me"), false, none.clone());
        register("am-chat", Provider::Claude, "C:\\p", account, None, false, none);
        let q = AskQuestion { text: "Which?".into(), options: vec!["Red".into(), "Blue".into()], multi: false, list_at: None };
        let ask = Ask::question(vec![q], false);
        let mine = answer_message("am-me", "am-kid", &ask, &["2".into()]).unwrap();
        assert_eq!(mine, "Answers to your question: \"Which?\": Blue", "its own helper knows its parent");
        let theirs = answer_message("am-me", "am-chat", &ask, &["2".into()]).unwrap();
        assert_eq!(theirs, "[From session am-me via Luna] Answers to your question: \"Which?\": Blue");
        assert!(answer_message("am-me", "am-chat", &ask, &[]).is_err(), "the count holds either way");
        for id in ["am-kid", "am-chat", "am-me"] {
            forget(id);
        }
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
        let keys = presses_for(Provider::Claude, &ask, &["2".into()]).unwrap();
        assert_eq!(keys, [Key::Seq("\x1b[B"), Key::Seq("\r")]);
        let own = presses_for(Provider::Claude, &ask, &["green".into()]).unwrap();
        assert_eq!(own, [Key::Seq("\x1b[B"), Key::Seq("\x1b[B"), Key::Text("green".into()), Key::Seq("\r")]);
        assert!(presses_for(Provider::Claude, &ask, &[]).is_err());
        // A chat's permission is never pressed for: the chat is the person's.
        let perm = Ask::permission(Some("Bash".into()), Some("ls".into()));
        assert!(presses_for(Provider::Codex, &perm, &[]).is_err());
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
        let keys = presses_for(Provider::Codex, &ask, &["3".into()]).unwrap();
        assert_eq!(keys, [Key::Seq("\x1b[B"), Key::Seq("\x1b[B"), Key::Seq("\r")]);
        assert!(presses_for(Provider::Codex, &ask, &["trust them all".into()]).is_err(), "no words of one's own");

        // From where the cursor is, not from the top.
        let moved = screen.replace("› 1.", "  1.").replace("  2. Trust", "› 2. Trust");
        let ask = codex_list_on(&moved).unwrap();
        let keys = presses_for(Provider::Codex, &ask, &["Review hooks".into()]).unwrap();
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
        assert!(refused(drain_lets_answer("s", None, None, plain, None)).contains("no question or permission pending"));
        // A structured answer goes to what is held.
        assert_eq!(drain_lets_answer("s", Some(&ask(None, true)), None, plain, None), Ok(()));
        assert_eq!(drain_lets_answer("s", Some(&ask(None, false)), None, &["yes".to_string()], None), Ok(()));
        // Not with what a terminal takes as a prompt in it.
        assert!(refused(drain_lets_answer("s", Some(&ask(None, true)), None, plain, Some("do this instead"))).contains("a note"));
        assert_eq!(drain_lets_answer("s", Some(&ask(None, true)), None, plain, Some("  ")), Ok(()));
        assert!(refused(drain_lets_answer("s", Some(&ask(None, true)), None, &["a\rnew prompt".to_string()], None)).contains("one line"));
        // A background helper's has an id, and it is to be named.
        assert!(refused(drain_lets_answer("s", Some(&ask(Some("a1"), true)), None, plain, None)).contains("a1"));
        assert!(refused(drain_lets_answer("s", Some(&ask(Some("a1"), true)), Some("a0"), plain, None)).contains("a1"));
        assert_eq!(drain_lets_answer("s", Some(&ask(Some("a1"), true)), Some("a1"), plain, None), Ok(()));
    }

    #[test]
    fn a_permission_reaches_only_the_callers_own_helpers() {
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
            assert_eq!(answer_reach(own, full, Provider::Claude, Some(&question), None), Ok(()));
            assert_eq!(answer_reach(own, full, Provider::Claude, None, None), Ok(()), "nothing known pending yet");
            for (held, allow) in [
                (Some(&permission), Some(true)),
                (Some(&permission), Some(false)),
                (Some(&permission), None),
                (None, Some(true)),
                (Some(&dialog), None),
            ] {
                let r = answer_reach(own, full, Provider::Codex, held, allow);
                assert_eq!(r.is_ok(), own, "{:?} allow {allow:?}: {r:?}", held.map(|a| a.kind));
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
    fn an_answer_in_ones_own_words_typed_into_a_chat_is_held_to_what_may_be_typed_there() {
        let mode = |m: &str| serde_json::json!({ "permissionMode": m });
        register("rn-asks", Provider::Claude, "C:/p", "C:/acc/one", None, true, mode("default"));
        register("rn-full", Provider::Claude, "C:/p", "C:/acc/one", None, true, mode("bypassPermissions"));
        register("rn-chat", Provider::Claude, "C:/p", "C:/acc/one", None, false, mode("default"));
        // Its dialog gone, the words are a prompt in the composer: a command there
        // is refused before any key goes in, whoever asks.
        for caller in ["rn-asks", "rn-full"] {
            for words in ["! rm -rf build", "  /permissions :workspace"] {
                let e = answer(caller, "rn-chat", None, &[words.to_string()], None, None, false).err().unwrap();
                assert!(e.contains("not a prompt"), "{caller} {words:?}: {e}");
            }
        }
        // A permission in the person's chat is not an agent's to settle.
        let e = answer("rn-full", "rn-chat", None, &[], Some(true), None, false).err().unwrap();
        assert!(e.contains("only to sessions you spawned"), "{e}");
        // Plain words pass those checks (and stop at the session not running).
        let e = answer("rn-asks", "rn-chat", None, &["Blue".to_string()], None, None, false).err().unwrap();
        assert!(e.contains("is not running"), "{e}");
        for id in ["rn-chat", "rn-asks", "rn-full"] {
            forget(id);
        }
    }
}
