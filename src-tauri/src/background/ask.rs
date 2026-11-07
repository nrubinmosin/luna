//! What is read of a helper and asked of it: its screen and transcript, the
//! permissions and questions its CLI holds, and the answers to them.

use super::*;

/// The last lines of what it did, in place of a screen.
pub fn screen(id: &str, lines: usize) -> Option<String> {
    let r = lock();
    let e = r.entries.get(id)?;
    let from = e.log.len().saturating_sub(lines);
    let mut out: Vec<String> = e.log.iter().skip(from).cloned().collect();
    if let Some(err) = &e.error {
        out.push(format!("last error: {}", one_line(err)));
    }
    Some(out.join("\n"))
}

/// Its transcript (Claude Code) or rollout (Codex), once the first turn has
/// named its conversation.
pub fn transcript(id: &str) -> Option<std::path::PathBuf> {
    let (provider, account, session, cwd, folder) = {
        let r = lock();
        let e = r.entries.get(id)?;
        (e.spec.provider, e.spec.account_path.clone(), e.session.clone()?, e.cwd.clone(), e.spec.folder.clone())
    };
    match provider {
        Provider::Claude => Some(crate::claude::session::transcript_file(&account, cwd.as_deref().unwrap_or(&folder), &session)),
        Provider::Codex => crate::codex::session::rollout_of_thread(&account, &session),
    }
}

/// The conversation's id as the CLI knows it.
pub fn session_id(id: &str) -> Option<String> {
    lock().entries.get(id)?.session.clone()
}

/// Where it works: the worktree its first turn made, or its folder.
pub fn working_dir(id: &str) -> Option<String> {
    let r = lock();
    let e = r.entries.get(id)?;
    Some(e.cwd.clone().unwrap_or_else(|| e.spec.folder.clone()))
}

pub fn ask(id: &str) -> Option<Ask> {
    lock().entries.get(id)?.held.iter().find(|h| h.answer.is_none()).map(|h| h.ask.clone())
}

/// `luna_permission`: the CLI asks before a tool runs, or carries the model's
/// `AskUserQuestion`. Held until `luna_answer`, the turn's end or the timeout;
/// the answer is the JSON the CLI takes back.
pub fn permission(id: &str, args: &Value) -> Value {
    let tool = args["tool_name"].as_str().unwrap_or("").to_string();
    let input = args["input"].clone();
    let mut ask = if tool == "AskUserQuestion" {
        Ask::from_claude_questions(&input).unwrap_or_else(|| Ask::from_claude_tool(&tool, &input))
    } else {
        Ask::from_claude_tool(&tool, &input)
    };
    let serial = {
        let mut r = lock();
        let serial = r.next_serial;
        r.next_serial += 1;
        let Some(e) = r.entries.get_mut(id) else {
            return json!({ "behavior": "deny", "message": "Luna does not know this session." });
        };
        ask.id = Some(format!("a{serial}"));
        note(e, &format!("waits: {}", ask.line()));
        e.held.push_back(Held { serial, ask, input, answer: None });
        r.asked.insert(serial, id.to_string());
        serial
    };
    changed();
    let deadline = std::time::Instant::now() + ASK_TIMEOUT;
    let (m, cv) = reg();
    let mut r = m.lock().unwrap_or_else(|e| e.into_inner());
    let answer = loop {
        let held = r.entries.get_mut(id).filter(|e| e.run.is_some()).and_then(|e| e.held.iter_mut().find(|h| h.serial == serial));
        match held {
            Some(h) => {
                if let Some(a) = h.answer.take() {
                    break Some(a);
                }
            }
            // Killed, deleted, or its turn is gone: nobody will answer.
            None => break None,
        }
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            break None;
        }
        r = cv.wait_timeout(r, left).unwrap_or_else(|e| e.into_inner()).0;
    };
    if let Some(e) = r.entries.get_mut(id) {
        e.held.retain(|h| h.serial != serial);
    }
    drop(r);
    changed();
    answer.unwrap_or_else(|| json!({ "behavior": "deny", "message": "Nobody answered in time; go on without it or ask in your reply." }))
}

/// The agent's answer to what the helper holds: `answers` for a question
/// (an option's number or label, or words of its own, one per question;
/// several numbers apart by commas where more than one may be picked),
/// `allow` for a permission, `note` as the reason for a refusal.
///
/// `ask_id` says which of what it holds this answers: two responders who saw
/// the same ask must not have the slower one's `allow` land on the next. One no
/// longer open is refused. Without it the answer goes through only when a
/// single ask is held, as it did when there could be only one: with several,
/// nothing says it was written on the first.
///
/// `may`: whether the caller may give this answer to the ask it lands on
/// (agents.rs `answer_reach`), asked of that very ask under the lock it is
/// recorded under, so that one looked at before is never the one let through.
pub fn answer(
    id: &str,
    ask_id: Option<&str>,
    answers: &[String],
    allow: Option<bool>,
    note_text: Option<&str>,
    may: impl FnOnce(&Ask) -> Result<(), String>,
) -> Result<(), String> {
    let mut r = lock();
    let given = ask_id.is_some_and(|want| {
        want.strip_prefix('a').and_then(|n| n.parse::<u64>().ok()).and_then(|n| r.asked.get(&n)).is_some_and(|to| to == id)
    });
    let e = r.entries.get_mut(id).ok_or("no such helper")?;
    let mut open = e.held.iter_mut().filter(|h| h.answer.is_none());
    let h = match ask_id {
        Some(want) => open.find(|h| h.ask.id.as_deref() == Some(want)).ok_or_else(|| {
            if given {
                format!("ask {want} of helper {id} is no longer open: it was answered or went with its turn. Its `ask` in luna_list says what it holds now")
            } else {
                format!("helper {id} has had no ask {want} (since Luna's core last started): the id is unknown. Its `ask` in luna_list has the id of what it holds now, if anything")
            }
        })?,
        None => {
            let first = open.next().ok_or_else(|| format!("helper {id} is not waiting on anything"))?;
            if open.next().is_some() {
                return Err(format!(
                    "helper {id} holds several asks at once: say which with askId. The first is {}: it {}",
                    first.ask.id.as_deref().unwrap_or("?"),
                    first.ask.line()
                ));
            }
            first
        }
    };
    may(&h.ask)?;
    let reply = build_answer(&h.ask, &h.input, answers, allow, note_text)?;
    h.answer = Some(reply);
    drop(r);
    reg().1.notify_all();
    Ok(())
}

fn build_answer(ask: &Ask, input: &Value, answers: &[String], allow: Option<bool>, note_text: Option<&str>) -> Result<Value, String> {
    if ask.kind == "permission" {
        return match allow {
            Some(true) => Ok(json!({ "behavior": "allow", "updatedInput": input })),
            Some(false) => Ok(json!({ "behavior": "deny", "message": note_text.unwrap_or("The user declined.") })),
            None => Err("this is a permission: answer with allow true or false".into()),
        };
    }
    if allow == Some(false) {
        return Ok(json!({ "behavior": "deny", "message": note_text.unwrap_or("The user declined to answer.") }));
    }
    if answers.len() != ask.questions.len() {
        return Err(format!("{} question(s), {} answer(s): give one answer per question", ask.questions.len(), answers.len()));
    }
    let mut picked = serde_json::Map::new();
    for (q, a) in ask.questions.iter().zip(answers) {
        picked.insert(q.text.clone(), Value::String(crate::agents::resolve_option(q, a)));
    }
    let mut updated = input.clone();
    if let Some(o) = updated.as_object_mut() {
        o.insert("answers".into(), Value::Object(picked));
    }
    Ok(json!({ "behavior": "allow", "updatedInput": updated }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_prompts_at_once_are_both_held_and_answered_in_order() {
        in_claimed_turn("two-asks");
        let ask = |tool: &'static str| {
            std::thread::spawn(move || permission("two-asks", &json!({ "tool_name": tool, "input": { "command": tool } })))
        };
        let held = || lock().entries.get("two-asks").unwrap().held.len();
        let until = |n: usize| {
            for _ in 0..200 {
                if held() == n {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            panic!("never {n} held");
        };
        let first = ask("Bash");
        until(1);
        let second = ask("Edit");
        until(2);
        let asked = super::ask("two-asks").unwrap();
        assert_eq!(asked.tool.as_deref(), Some("Bash"), "the one that came first");
        assert_eq!(turn_state("two-asks").unwrap().turn, crate::activity::Turn::Waiting);
        assert!(!turn_holds(&turn_state("two-asks").unwrap()), "held: waiting, keeping nothing awake");
        let stale = asked.id.clone().unwrap();
        answer("two-asks", Some(&stale), &[], Some(true), None, |_| Ok(())).unwrap();
        reg().1.notify_all();
        assert_eq!(first.join().unwrap()["behavior"], "allow", "the second did not cost it its answer");
        let next = super::ask("two-asks").unwrap();
        assert_eq!(next.tool.as_deref(), Some("Edit"), "and then the next");
        assert_ne!(next.id, asked.id);
        assert_eq!(turn_state("two-asks").unwrap().turn, crate::activity::Turn::Waiting, "still held on that one");
        // A second responder who also saw the first, its answer late.
        let late = answer("two-asks", Some(&stale), &[], Some(true), None, |_| Ok(())).unwrap_err();
        assert!(late.contains("no longer open"), "{late}");
        // An id it never had: none it was given, or another helper's.
        for never in ["a0", "nonsense"] {
            let e = answer("two-asks", Some(never), &[], Some(true), None, |_| Ok(())).unwrap_err();
            assert!(e.contains("unknown") && !e.contains("no longer open"), "{e}");
        }
        lock().asked.insert(u64::MAX, "another".into());
        assert!(answer("two-asks", Some(&format!("a{}", u64::MAX)), &[], Some(true), None, |_| Ok(())).unwrap_err().contains("unknown"));
        lock().asked.remove(&u64::MAX);
        assert_eq!(super::ask("two-asks").unwrap().id, next.id, "the next is untouched");
        answer("two-asks", None, &[], Some(false), Some("not that"), |_| Ok(())).unwrap();
        let no = second.join().unwrap();
        assert_eq!((no["behavior"].as_str(), no["message"].as_str()), (Some("deny"), Some("not that")));
        assert_eq!(super::ask("two-asks"), None);
        assert_eq!(turn_state("two-asks").unwrap().turn, crate::activity::Turn::Busy);
        lock().entries.remove("two-asks");
    }

    #[test]
    fn with_several_held_an_answer_says_which() {
        in_claimed_turn("which-ask");
        let ask = |tool: &'static str| std::thread::spawn(move || permission("which-ask", &json!({ "tool_name": tool, "input": {} })));
        let first = ask("Bash");
        for _ in 0..200 {
            if lock().entries.get("which-ask").unwrap().held.len() == 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let second = ask("Edit");
        for _ in 0..200 {
            if lock().entries.get("which-ask").unwrap().held.len() == 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let unsaid = answer("which-ask", None, &[], Some(true), None, |_| Ok(())).unwrap_err();
        assert!(unsaid.contains("askId") && unsaid.contains("Bash"), "says how, and what is first: {unsaid}");
        assert!(lock().entries.get("which-ask").unwrap().held.iter().all(|h| h.answer.is_none()), "and answers nothing");
        // One named out of turn is the one it means.
        let last = lock().entries.get("which-ask").unwrap().held.back().unwrap().ask.id.clone().unwrap();
        answer("which-ask", Some(&last), &[], Some(false), None, |_| Ok(())).unwrap();
        reg().1.notify_all();
        assert_eq!(second.join().unwrap()["behavior"], "deny");
        assert_eq!(super::ask("which-ask").unwrap().tool.as_deref(), Some("Bash"));
        // A single ask is answered with no id, as before.
        answer("which-ask", None, &[], Some(true), None, |_| Ok(())).unwrap();
        reg().1.notify_all();
        assert_eq!(first.join().unwrap()["behavior"], "allow");
        assert!(answer("which-ask", None, &[], Some(true), None, |_| Ok(())).unwrap_err().contains("not waiting"));
        lock().entries.remove("which-ask");
    }

    #[test]
    fn an_ask_says_its_id_to_whoever_is_to_answer_it() {
        in_claimed_turn("id-ask");
        let held = std::thread::spawn(|| permission("id-ask", &json!({ "tool_name": "Bash", "input": { "command": "ls" } })));
        for _ in 0..200 {
            if ask("id-ask").is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let shown = serde_json::to_value(ask("id-ask").unwrap()).unwrap();
        assert!(shown["id"].as_str().is_some_and(|i| !i.is_empty()), "{shown}");
        kill("id-ask");
        let _ = held.join();
        lock().entries.remove("id-ask");
        assert!(serde_json::to_value(Ask::permission(None, None)).unwrap().get("id").is_none(), "a terminal's has none");
    }

    #[test]
    fn an_answer_is_shaped_as_the_cli_takes_it() {
        let input = json!({ "questions": [{ "question": "Which color?", "options": [{ "label": "Red" }, { "label": "Blue" }] }] });
        let ask = Ask::from_claude_questions(&input).unwrap();
        let a = build_answer(&ask, &input, &["2".into()], None, None).unwrap();
        assert_eq!(a["behavior"], "allow");
        assert_eq!(a["updatedInput"]["answers"]["Which color?"], "Blue");
        assert_eq!(a["updatedInput"]["questions"], input["questions"], "the input goes back with the answers added");
        assert!(build_answer(&ask, &input, &[], None, None).is_err(), "one answer per question");
        let own = build_answer(&ask, &input, &["green, actually".into()], None, None).unwrap();
        assert_eq!(own["updatedInput"]["answers"]["Which color?"], "green, actually");

        let cmd = json!({ "command": "rm -rf build" });
        let perm = Ask::from_claude_tool("Bash", &cmd);
        assert_eq!(build_answer(&perm, &cmd, &[], Some(true), None).unwrap(), json!({ "behavior": "allow", "updatedInput": cmd }));
        let no = build_answer(&perm, &cmd, &[], Some(false), Some("not now")).unwrap();
        assert_eq!((no["behavior"].as_str(), no["message"].as_str()), (Some("deny"), Some("not now")));
        assert!(build_answer(&perm, &cmd, &[], None, None).is_err());
    }

    #[test]
    fn a_background_answer_is_checked_against_the_ask_it_lands_on() {
        use crate::agents::{answer, forget, register};
        let mode = |m: &str| serde_json::json!({ "permissionMode": m });
        register("ba-edits", Provider::Claude, "C:/p", "C:/acc/one", None, true, mode("acceptEdits"));
        register("ba-kid", Provider::Claude, "C:/p", "C:/acc/one", Some("ba-edits"), false, mode("default"));
        lock().entries.insert("ba-kid".into(), entry(Provider::Claude));
        let hold = |serial: u64, ask: Ask| {
            let mut r = lock();
            r.asked.insert(serial, "ba-kid".into());
            let ask = Ask { id: Some(format!("a{serial}")), ..ask };
            r.entries.get_mut("ba-kid").unwrap().held.push_back(Held { serial, ask, input: Value::Null, answer: None });
        };
        let answered = |serial: u64| lock().entries["ba-kid"].held.iter().find(|h| h.serial == serial).is_some_and(|h| h.answer.is_some());
        let yes = |ask_id: Option<&str>| answer("ba-edits", "ba-kid", ask_id, &[], Some(true), None, None, false).map(|_| ());

        // An edit held first, a command behind it: the first passes for an
        // acceptEdits caller, the command does not, named by its id.
        hold(901, Ask::permission(Some("Edit".into()), None));
        hold(902, Ask::permission(Some("Bash".into()), Some("rm -rf build".into())));
        let e = yes(Some("a902")).unwrap_err();
        assert!(e.contains("approving Bash"), "{e}");
        assert!(!answered(902));
        // Without an id, with both held: which one is not said, so neither.
        assert!(yes(None).unwrap_err().contains("several asks"));
        yes(Some("a901")).unwrap();
        assert!(answered(901));
        // The command alone now: without an id it is the one checked.
        assert!(yes(None).unwrap_err().contains("approving Bash"));
        assert!(!answered(902));
        // Refusing it stays the caller's.
        answer("ba-edits", "ba-kid", Some("a902"), &[], Some(false), Some("not that"), None, false).unwrap();
        assert!(answered(902));

        // A model's question held first: answering it takes no privilege, and
        // does not let a command behind it through.
        let question = Ask::question(
            vec![crate::agents::AskQuestion { text: "Which?".into(), options: vec!["A".into()], multi: false, list_at: None }],
            true,
        );
        hold(903, question);
        hold(904, Ask::permission(Some("Bash".into()), None));
        assert!(yes(Some("a904")).unwrap_err().contains("approving Bash"));
        assert!(!answered(904));
        answer("ba-edits", "ba-kid", Some("a903"), &["A".to_string()], None, None, None, false).unwrap();
        assert!(answered(903));

        lock().entries.remove("ba-kid");
        forget("ba-kid");
        forget("ba-edits");
    }
}
