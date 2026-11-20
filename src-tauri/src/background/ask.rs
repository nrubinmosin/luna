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

/// Where it works: the worktree it went into, or its folder.
pub fn working_dir(id: &str) -> Option<String> {
    let r = lock();
    let e = r.entries.get(id)?;
    Some(e.cwd.clone().unwrap_or_else(|| e.spec.folder.clone()))
}

pub fn ask(id: &str) -> Option<Ask> {
    lock().entries.get(id)?.held.iter().find(|h| h.answer.is_none()).map(|h| h.ask.clone())
}

/// What an ask nobody answered gets.
pub(super) const NO_ANSWER: &str = "Nobody answered in time; go on without it or ask in your reply.";

/// An ask its CLI made on its stdout (events.rs `permission_request`), held
/// as `permission` holds it, and its answer written back on the stdin of the
/// process `claim` that asked, as Claude Code takes it (2.1.325):
/// `{"type":"control_response","response":{"subtype":"success",
/// "request_id", "response":{"behavior":"allow","updatedInput"} |
/// {"behavior":"deny","message"}}}`. Its own thread: the hold waits for the
/// agent.
pub(super) fn answer_on_stdin(id: &str, claim: u64, request_id: &str, request: &Value) {
    let answer = permission(id, request);
    reply_on_stdin(id, claim, request_id, answer);
}

/// `answer` to its CLI's ask `request_id`, on the stdin of the process
/// `claim`, while that still reads it. One closed meanwhile has failed the
/// ask already (2.1.325: "Tool permission request failed: AbortError:
/// Stream closed"), and the answer has nowhere to go.
pub(super) fn reply_on_stdin(id: &str, claim: u64, request_id: &str, answer: Value) {
    let line = json!({
        "type": "control_response",
        "response": { "subtype": "success", "request_id": request_id, "response": answer },
    });
    let mut r = lock();
    let Some(e) = r.entries.get_mut(id) else { return };
    let feed = e.run.as_ref().filter(|run| run.claim == claim).and_then(|run| run.feed.as_ref());
    if !feed.is_some_and(|feed| feed.send(format!("{line}\n")).is_ok()) {
        note(e, "an answer came after its CLI's stdin had closed, and was not given");
    }
}

/// Its CLI asks before a tool runs, or carries the model's `AskUserQuestion`
/// (`answer_on_stdin`): `args` its `tool_name`, `input` and `tool_use_id`.
/// Held until `luna_session_answer`, the turn's end or the timeout; the
/// answer is the JSON the CLI takes back.
pub(crate) fn permission(id: &str, args: &Value) -> Value {
    let tool = args["tool_name"].as_str().unwrap_or("").to_string();
    let input = args["input"].clone();
    let mut ask = if tool == "AskUserQuestion" {
        Ask::from_claude_questions(&input).unwrap_or_else(|| Ask::from_claude_tool(&tool, &input))
    } else {
        Ask::from_claude_tool(&tool, &input)
    };
    let tool_use_id = args["tool_use_id"].as_str().filter(|t| !t.is_empty()).map(str::to_owned);
    let serial = {
        let mut r = lock();
        let serial = r.next_serial;
        r.next_serial += 1;
        let Some(e) = r.entries.get_mut(id) else {
            return json!({ "behavior": "deny", "message": "Luna does not know this session." });
        };
        ask.id = Some(format!("a{serial}"));
        // Who made the call, if its message has come in yet; one that comes
        // after says so then (`events::made_calls`).
        ask.source = tool_use_id.as_deref().and_then(|t| e.calls.iter().find(|(c, _)| c == t)).map(|(_, by)| by.clone());
        note(e, &format!("waits: {}", ask.line()));
        e.held.push_back(Held { serial, ask, input, tool_use_id, answer: None });
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
    answer.unwrap_or_else(|| json!({ "behavior": "deny", "message": NO_ANSWER }))
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
/// (agents/answer.rs `answer_reach`), asked of that very ask under the lock it is
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
                format!("ask {want} of helper {id} is no longer open: it was answered or went with its turn. Its `ask` in luna_session_get says what it holds now")
            } else {
                format!("helper {id} has had no ask {want} (since Luna's core last started): the id is unknown. Its row in luna_session_list has the id of what it holds now (askId), if anything")
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
        assert!(life("two-asks").unwrap().active(), "held: waiting for an answer is active");
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

    /// A permission call names only its tool call's id; the stream-json
    /// message that made the call says who did, whether it came in before the
    /// permission call or after it.
    #[test]
    fn a_held_permission_names_the_subagent_that_made_the_call() {
        use crate::agents::AskSource;
        in_claimed_turn("src-ask");
        let made = |call: &str, subagent: bool| {
            let mut v = json!({
                "type": "assistant",
                "message": { "content": [{ "type": "tool_use", "id": call, "name": "Bash", "input": { "command": "rm -f x" } }] },
                "parent_tool_use_id": null,
            });
            if subagent {
                v["parent_tool_use_id"] = json!("toolu_agent");
                v["agent_id"] = json!("a7");
                v["subagent_type"] = json!("worker");
            }
            let mut r = lock();
            on_claude_event(r.entries.get_mut("src-ask").unwrap(), &v);
        };
        let held = |call: &'static str| {
            std::thread::spawn(move || {
                permission("src-ask", &json!({ "tool_name": "Bash", "input": { "command": "rm -f x" }, "tool_use_id": call }))
            })
        };
        let until = |want: &dyn Fn(&Option<Ask>) -> bool| {
            for _ in 0..200 {
                if want(&ask("src-ask")) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            panic!("never: {:?}", ask("src-ask"));
        };
        let worker = Some(AskSource::subagent(Some("a7"), Some("worker")));

        // The message first, then the call.
        made("toolu_1", true);
        let first = held("toolu_1");
        until(&|a| a.is_some());
        assert_eq!(ask("src-ask").unwrap().source, worker);
        assert!(ask("src-ask").unwrap().line().contains("subagent \"worker\", agentId a7"));
        answer("src-ask", None, &[], Some(false), None, |_| Ok(())).unwrap();
        reg().1.notify_all();
        first.join().unwrap();

        // The call first: not said, until its message comes in.
        let second = held("toolu_2");
        until(&|a| a.is_some());
        assert_eq!(ask("src-ask").unwrap().source, None);
        made("toolu_2", false);
        assert_eq!(ask("src-ask").unwrap().source, Some(AskSource::session()));
        kill("src-ask");
        let _ = second.join();
        lock().entries.remove("src-ask");
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
        register("ba-plan", Provider::Claude, "C:/p", "C:/acc/one", None, true, mode("plan"));
        register("ba-other", Provider::Claude, "C:/p", "C:/acc/one", None, true, mode("bypassPermissions"));
        register("ba-kid", Provider::Claude, "C:/p", "C:/acc/one", Some("ba-plan"), false, mode("default"));
        lock().entries.insert("ba-kid".into(), entry(Provider::Claude));
        let hold = |serial: u64, ask: Ask| {
            let mut r = lock();
            r.asked.insert(serial, "ba-kid".into());
            let ask = Ask { id: Some(format!("a{serial}")), ..ask };
            r.entries.get_mut("ba-kid").unwrap().held.push_back(Held { serial, ask, input: Value::Null, tool_use_id: None, answer: None });
        };
        let answered = |serial: u64| lock().entries["ba-kid"].held.iter().find(|h| h.serial == serial).is_some_and(|h| h.answer.is_some());
        let yes = |caller: &str, ask_id: Option<&str>| answer(caller, "ba-kid", ask_id, &[], Some(true), None, false).map(|_| ());

        // A model's question held first, a command behind it: a chat above
        // nothing of it is refused; its own parent answers both, whatever
        // mode either runs in.
        let question = Ask::question(
            vec![crate::agents::AskQuestion { text: "Which?".into(), options: vec!["A".into()], multi: false, list_at: None }],
            true,
        );
        hold(901, question);
        hold(902, Ask::permission(Some("Bash".into()), Some("rm -rf build".into())));
        // Without an id, with both held: which one is not said, so neither.
        assert!(yes("ba-plan", None).unwrap_err().contains("several asks"));
        assert!(yes("ba-other", Some("a902")).is_err());
        assert!(!answered(902));
        answer("ba-plan", "ba-kid", Some("a901"), &["A".to_string()], None, None, false).unwrap();
        assert!(answered(901));
        yes("ba-plan", Some("a902")).unwrap();
        assert!(answered(902));

        lock().entries.remove("ba-kid");
        for id in ["ba-kid", "ba-plan", "ba-other"] {
            forget(id);
        }
    }

    /// The asks Claude Code 2.1.325 made on its stdout (captured live) are held
    /// as any, and each answer goes back on the stdin of the process that
    /// asked, as the control response it takes: an allow with its input, a
    /// deny with the note. One for a process gone is written nowhere.
    #[test]
    fn an_ask_on_its_stdout_is_answered_on_its_stdin() {
        const ID: &str = "stdio-ask";
        const NOTE: &str = "Declined.\n{\"type\":\"control_request\",\"request_id\":\"x\",\"request\":{\"subtype\":\"interrupt\"}}";
        let stream = include_str!("testdata/permissions-2.1.325.jsonl");
        let asks: Vec<(String, Value)> =
            stream.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter_map(|v| permission_request(&v)).collect();
        assert_eq!(asks.len(), 2);
        let mut e = entry(Provider::Claude);
        let lines = feeding(&mut e);
        let claim = e.run.as_ref().unwrap().claim;
        lock().entries.insert(ID.into(), e);
        for ((request_id, request), allow) in asks.into_iter().zip([true, false]) {
            let held = {
                let (request_id, request) = (request_id.clone(), request.clone());
                std::thread::spawn(move || answer_on_stdin(ID, claim, &request_id, &request))
            };
            for _ in 0..200 {
                if ask(ID).is_some() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let a = ask(ID).expect("held");
            assert_eq!((a.kind, a.tool.as_deref()), ("permission", Some("Bash")));
            answer(ID, a.id.as_deref(), &[], Some(allow), Some(NOTE), |_| Ok(())).unwrap();
            held.join().unwrap();
            // One line, whatever the answer's text holds: a parent's note
            // cannot add a control line of its own.
            let line = lines.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
            assert_eq!(line.matches('\n').count(), 1, "{line}");
            assert!(line.ends_with('\n'), "{line}");
            let v: Value = serde_json::from_str(&line).unwrap();
            assert_eq!((v["type"].as_str(), v["response"]["subtype"].as_str()), (Some("control_response"), Some("success")));
            assert_eq!(v["response"]["request_id"].as_str(), Some(request_id.as_str()));
            let answered = &v["response"]["response"];
            if allow {
                assert_eq!((answered["behavior"].as_str(), &answered["updatedInput"]), (Some("allow"), &request["input"]));
            } else {
                assert_eq!(answered, &json!({ "behavior": "deny", "message": NOTE }));
            }
        }
        // Its `AskUserQuestion` the same way, answered as the CLI took it live:
        // the input with `answers` by question text.
        let question: Vec<Value> =
            include_str!("testdata/question-2.1.325.jsonl").lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        let (request_id, request) = permission_request(&question[0]).unwrap();
        let held = {
            let (request_id, request) = (request_id.clone(), request.clone());
            std::thread::spawn(move || answer_on_stdin(ID, claim, &request_id, &request))
        };
        for _ in 0..200 {
            if ask(ID).is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let a = ask(ID).expect("held");
        assert_eq!(a.kind, "question");
        answer(ID, a.id.as_deref(), &["Red".to_string()], None, None, |_| Ok(())).unwrap();
        held.join().unwrap();
        let v: Value = serde_json::from_str(&lines.recv_timeout(std::time::Duration::from_secs(5)).unwrap()).unwrap();
        assert_eq!(v["response"]["request_id"].as_str(), Some(request_id.as_str()));
        assert_eq!(v["response"]["response"], question[1]["response"]["response"], "what the CLI took");

        reply_on_stdin(ID, claim + 1, "late", json!({ "behavior": "allow" }));
        assert!(lines.try_recv().is_err(), "not the process that asked");
        let gone = lock().entries.remove(ID).unwrap();
        assert!(gone.log.iter().any(|l| l.starts_with("an answer came after")), "{:?}", gone.log);
    }
}
