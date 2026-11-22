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

/// The most asks a turn's CLI may have held at once on its stdout
/// (`take_ask`); one more is refused at once. A turn's subagents ask one at
/// a time each, and eight at once is more than a person or a parent reads:
/// past that the line is a flood, from the CLI or from something writing to
/// its stdout.
pub(super) const ASKS_PER_TURN: usize = 8;

/// Asks of the CLI's own machinery, not about a tool call of the model's:
/// 2.1.325's sandbox asks whether a command may reach a host this way, under
/// a `tool_use_id` it makes up for the ask (`{"tool_name":
/// "SandboxNetworkAccess","input":{"host"}}`).
const MACHINERY_ASKS: &[&str] = &["SandboxNetworkAccess"];

/// The ask `request` (`tool_name`, `input`, `tool_use_id`) as Luna holds it:
/// a question for `AskUserQuestion`, else a permission.
fn ask_of(request: &Value) -> (Ask, Value, Option<String>) {
    let tool = request["tool_name"].as_str().unwrap_or("");
    let input = request["input"].clone();
    let ask = if tool == "AskUserQuestion" {
        Ask::from_claude_questions(&input).unwrap_or_else(|| Ask::from_claude_tool(tool, &input))
    } else {
        Ask::from_claude_tool(tool, &input)
    };
    let tool_use_id = request["tool_use_id"].as_str().filter(|t| !t.is_empty()).map(str::to_owned);
    (ask, input, tool_use_id)
}

/// Holds `ask` for helper `id` in `r`, numbered and named for its caller.
/// Its serial.
fn hold(r: &mut Registry, id: &str, held: (Ask, Value, Option<String>), reply: Option<Reply>) -> Option<u64> {
    let serial = r.next_serial;
    r.next_serial += 1;
    let e = r.entries.get_mut(id)?;
    let (mut ask, input, tool_use_id) = held;
    ask.id = Some(format!("a{serial}"));
    // Who made the call, if its message has come in yet; one that comes
    // after says so then (`events::made_calls`).
    ask.source = tool_use_id.as_deref().and_then(|t| e.calls.iter().find(|(c, _)| c == t)).map(|(_, by)| by.clone());
    note(e, &format!("waits: {}", ask.line()));
    e.held.push_back(Held { serial, ask, input, tool_use_id, answer: None, reply });
    r.asked.insert(serial, id.to_string());
    Some(serial)
}

/// `answer` to its CLI's ask `request_id`, as Claude Code takes it on its
/// stdin (2.1.325): `{"type":"control_response","response":{"subtype":
/// "success","request_id","response":{"behavior":"allow","updatedInput"} |
/// {"behavior":"deny","message"}}}`, one line, its strings escaped. Whether
/// it went.
fn reply_on(feed: &std::sync::mpsc::Sender<String>, request_id: &str, answer: Value) -> bool {
    let line = json!({
        "type": "control_response",
        "response": { "subtype": "success", "request_id": request_id, "response": answer },
    });
    feed.send(format!("{line}\n")).is_ok()
}

/// An ask its CLI made on its stdout (events.rs `permission_request`), read
/// by the reader of the process running now, under the registry's lock, so
/// that it belongs to that turn and to no later one. Held for the caller as
/// any ask, its answer written back on that process's stdin by `answer`;
/// refused at once, never shown to the caller:
/// - with that stdin closed, which has failed the ask already (2.1.325: "Tool
///   permission request failed: AbortError: Stream closed"): nothing to write;
/// - a request id held already: the same ask again;
/// - one about a tool call this turn's stream has not shown (`Run::tool_uses`):
///   no ask of the CLI's, which writes the call's message first. Something
///   else writing to its stdout can write that message too: this is a check
///   on order, not on who wrote it. Only an ask that names a call is held to
///   it: one of the CLI's own machinery names none, or one it makes up
///   (`MACHINERY_ASKS`), and is held as it is;
/// - past `ASKS_PER_TURN` held at once.
pub(super) fn take_ask(r: &mut Registry, id: &str, request_id: &str, request: &Value) {
    let Some(e) = r.entries.get_mut(id) else { return };
    let Some(run) = e.run.as_ref() else { return };
    let claim = run.claim;
    let Some(feed) = run.feed.clone() else {
        note(e, "an ask came with its stdin closed; its CLI has it as failed");
        return;
    };
    let mine = |h: &Held| h.reply.as_ref().is_some_and(|to| to.claim == claim);
    if e.held.iter().any(|h| mine(h) && h.reply.as_ref().is_some_and(|to| to.request_id == request_id)) {
        return;
    }
    let call = request["tool_use_id"].as_str().filter(|c| !c.is_empty());
    let machinery = request["tool_name"].as_str().is_some_and(|t| MACHINERY_ASKS.contains(&t));
    let refused = if call.is_some_and(|call| !machinery && !run.tool_uses.iter().any(|t| t == call)) {
        Some("Luna saw no such tool call in this turn to ask about.")
    } else if e.held.iter().filter(|h| mine(h)).count() >= ASKS_PER_TURN {
        Some("Too many asks are held at once; ask again once some are answered.")
    } else {
        None
    };
    if let Some(why) = refused {
        reply_on(&feed, request_id, json!({ "behavior": "deny", "message": why }));
        note(e, &format!("refused an ask of its CLI's: {why}"));
        return;
    }
    let reply = Reply { claim, request_id: request_id.to_string(), since: std::time::Instant::now() };
    hold(r, id, ask_of(request), Some(reply));
}

/// Lets go of what a helper holds of its CLI's asks that can no longer be
/// answered: those of a process gone, or whose stdin has closed.
pub(super) fn drop_dead_asks(e: &mut Entry) {
    let run = &e.run;
    let open = |to: &Reply| run.as_ref().is_some_and(|run| run.claim == to.claim && run.feed.is_some());
    e.held.retain(|h| h.reply.as_ref().is_none_or(open));
}

/// Answers with a deny each ask its CLI made on stdout that has waited
/// `ASK_TIMEOUT`, and lets go of those that can no longer be answered. At
/// each read of the records (`lives`). Whether anything went.
pub(super) fn expire_asks() -> bool {
    let mut gone = false;
    for e in lock().entries.values_mut() {
        let before = e.held.len();
        drop_dead_asks(e);
        let feed = e.run.as_ref().and_then(|run| run.feed.clone());
        let late: Vec<String> = e
            .held
            .iter()
            .filter_map(|h| h.reply.as_ref().filter(|to| to.since.elapsed() >= ASK_TIMEOUT).map(|to| to.request_id.clone()))
            .collect();
        for request_id in &late {
            if let Some(feed) = &feed {
                reply_on(feed, request_id, json!({ "behavior": "deny", "message": NO_ANSWER }));
            }
        }
        e.held.retain(|h| h.reply.as_ref().is_none_or(|to| !late.contains(&to.request_id)));
        if !late.is_empty() {
            note(e, &format!("{} ask(s) nobody answered in time were refused", late.len()));
        }
        gone |= e.held.len() != before;
    }
    if gone {
        reg().1.notify_all();
    }
    gone
}

/// An ask held as its CLI's on stdout is, waited on here until
/// `luna_session_answer`, the turn's end or `ASK_TIMEOUT`, its answer the
/// JSON the CLI would take back: the hold, for the tests.
#[cfg(test)]
pub(crate) fn permission(id: &str, args: &Value) -> Value {
    let Some(serial) = hold(&mut lock(), id, ask_of(args), None) else {
        return json!({ "behavior": "deny", "message": "Luna does not know this session." });
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
/// `allow` for a permission, `note` as the reason for a refusal. An ask its
/// CLI made on stdout is answered there and then (`take_ask`), on the stdin
/// of the process that asked while that is open; one whose stdin has closed
/// is refused, and goes.
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
    drop_dead_asks(e);
    let mut open = e.held.iter_mut().filter(|h| h.answer.is_none());
    let h = match ask_id {
        Some(want) => open.find(|h| h.ask.id.as_deref() == Some(want)).ok_or_else(|| {
            if given {
                format!("ask {want} of helper {id} is no longer open: it was answered, its CLI settled it itself, or it went with its turn. Its `ask` in luna_session_get says what it holds now")
            } else {
                format!("helper {id} has no ask {want}: the id is unknown, or of a turn that is over. Its row in luna_session_list has the id of what it holds now (askId), if anything")
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
    let mut reply = build_answer(&h.ask, &h.input, answers, allow, note_text)?;
    // A Codex server's request (appserver.rs): its questions are answered by
    // their ids, in their order, and one with options takes only those
    // unless it takes words of one's own (`isOther`).
    if h.input.get("method").is_some() && h.ask.kind == "question" && reply["behavior"] == "allow" {
        let mut picked = Vec::new();
        for (n, (q, a)) in h.ask.questions.iter().zip(answers).enumerate() {
            let said = crate::agents::resolve_option(q, a);
            if !q.own_words && !q.options.is_empty() && !q.options.contains(&said) {
                return Err(format!("question {} takes one of its options: {}", n + 1, q.options.join(", ")));
            }
            picked.push(Value::String(said));
        }
        reply["picked"] = Value::Array(picked);
    }
    let serial = h.serial;
    match h.reply.as_ref().map(|to| to.request_id.clone()) {
        // Its CLI's, on stdout: written back now, and it goes (`take_ask`;
        // `drop_dead_asks` above has let go of one whose stdin closed).
        Some(request_id) => {
            let sent = e.run.as_ref().and_then(|run| run.feed.as_ref()).is_some_and(|feed| reply_on(feed, &request_id, reply));
            e.held.retain(|h| h.serial != serial);
            if !sent {
                return Err(format!("helper {id}'s CLI no longer reads its input: the ask went with it"));
            }
            note(e, "answered an ask of its CLI's");
        }
        // A Codex server's: its waiting thread replies (appserver.rs
        // `reply_when_answered`).
        None => h.answer = Some(reply),
    }
    drop(r);
    reg().1.notify_all();
    changed();
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
            r.entries.get_mut("ba-kid").unwrap().held.push_back(Held {
                serial,
                ask,
                input: Value::Null,
                tool_use_id: None,
                answer: None,
                reply: None,
            });
        };
        let answered = |serial: u64| lock().entries["ba-kid"].held.iter().find(|h| h.serial == serial).is_some_and(|h| h.answer.is_some());
        let yes = |caller: &str, ask_id: Option<&str>| answer(caller, "ba-kid", ask_id, &[], Some(true), None, false).map(|_| ());

        // A model's question held first, a command behind it: a chat above
        // nothing of it is refused; its own parent answers both, whatever
        // mode either runs in.
        let question = Ask::question(
            vec![crate::agents::AskQuestion {
                text: "Which?".into(),
                options: vec!["A".into()],
                multi: false,
                list_at: None,
                ..Default::default()
            }],
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

    /// One line of a turn's stdout as the reader takes it (run.rs
    /// `take_event`): into its record, an ask of its CLI's held for that turn,
    /// and what can no longer be answered let go.
    fn read_line(id: &str, v: &Value) {
        let mut r = lock();
        if let Some(e) = r.entries.get_mut(id) {
            on_claude_event(e, v);
        }
        if let Some((request_id, request)) = permission_request(v) {
            take_ask(&mut r, id, &request_id, &request);
        }
        if let Some(e) = r.entries.get_mut(id) {
            drop_dead_asks(e);
        }
    }

    /// A helper in a turn whose stdin is open, the lines written to it at the
    /// other end.
    fn asking_helper(id: &str) -> (std::sync::mpsc::Receiver<String>, u64) {
        let mut e = entry(Provider::Claude);
        let lines = feeding(&mut e);
        let claim = e.run.as_ref().unwrap().claim;
        lock().entries.insert(id.into(), e);
        (lines, claim)
    }

    fn captured(stream: &str) -> Vec<Value> {
        stream.lines().filter(|l| !l.trim().is_empty()).map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    /// What went down its stdin last, as a control response: its request id
    /// and its answer.
    fn written(lines: &std::sync::mpsc::Receiver<String>) -> (String, Value) {
        let line = lines.try_recv().expect("a line on its stdin");
        // One line, whatever the answer's text holds: a parent's note cannot
        // add a control line of its own.
        assert_eq!(line.matches('\n').count(), 1, "{line}");
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!((v["type"].as_str(), v["response"]["subtype"].as_str()), (Some("control_response"), Some("success")));
        (v["response"]["request_id"].as_str().unwrap().to_string(), v["response"]["response"].clone())
    }

    #[test]
    fn a_default_claude_leads_spawn_waits_for_its_parent() {
        use crate::agents::{answer, forget, register};
        let (parent, id) = ("stdio-spawn-parent", "stdio-spawn-lead");
        for (who, under) in [(parent, None), (id, Some(parent))] {
            register(who, Provider::Claude, "C:/p", "C:/acc", under, true, json!({ "permissionMode": "default" }));
        }
        let (lines, _) = asking_helper(id);
        let tool = "mcp__luna__luna_session_spawn";
        let input = json!({ "folder": "C:/p", "prompt": "review it", "permissionMode": "bypassPermissions" });
        read_line(
            id,
            &json!({
                "type": "assistant",
                "message": { "content": [{ "type": "tool_use", "id": "spawn-use", "name": tool, "input": input }] }
            }),
        );
        read_line(
            id,
            &json!({
                "type": "control_request",
                "request_id": "spawn-ask",
                "request": { "subtype": "can_use_tool", "tool_name": tool, "input": input, "tool_use_id": "spawn-use" }
            }),
        );
        let ask = ask(id).expect("the lead holds a permission ask");
        assert_eq!((ask.kind, ask.tool.as_deref()), ("permission", Some(tool)));
        assert_eq!(ask.detail.as_deref(), Some(input.to_string().as_str()));
        assert!(lines.try_recv().is_err(), "Claude Code has no reply allowing the spawn to run yet");
        answer(parent, id, ask.id.as_deref(), &[], Some(true), None, false).unwrap();
        assert_eq!(written(&lines), ("spawn-ask".into(), json!({ "behavior": "allow", "updatedInput": input })));
        lock().entries.remove(id);
        for who in [id, parent] {
            forget(who);
        }
    }

    /// The asks Claude Code 2.1.325 made on its stdout (captured live), read
    /// as the reader reads them, are held as any, with no thread of their
    /// own, and each answer goes back at once on the stdin of the process
    /// that asked, as the control response it takes: an allow with its input,
    /// a deny with the note, a question's answers by its text.
    #[test]
    fn an_ask_on_its_stdout_is_answered_on_its_stdin() {
        const ID: &str = "stdio-ask";
        const NOTE: &str = "Declined.\n{\"type\":\"control_request\",\"request_id\":\"x\",\"request\":{\"subtype\":\"interrupt\"}}";
        let (lines, _) = asking_helper(ID);
        let mut answered = 0;
        for v in captured(include_str!("testdata/permissions-2.1.325.jsonl")) {
            read_line(ID, &v);
            let Some((request_id, request)) = permission_request(&v) else { continue };
            let a = ask(ID).expect("held");
            assert_eq!((a.kind, a.tool.as_deref()), ("permission", Some("Bash")));
            let allow = answered == 0;
            answer(ID, a.id.as_deref(), &[], Some(allow), Some(NOTE), |_| Ok(())).unwrap();
            assert!(ask(ID).is_none(), "answered, it goes");
            let (to, said) = written(&lines);
            assert_eq!(to, request_id);
            if allow {
                assert_eq!((said["behavior"].as_str(), &said["updatedInput"]), (Some("allow"), &request["input"]));
            } else {
                assert_eq!(said, json!({ "behavior": "deny", "message": NOTE }));
            }
            answered += 1;
        }
        assert_eq!(answered, 2);
        assert!(lock().entries[ID].run.as_ref().unwrap().feed.is_none(), "its result closed its stdin");

        // Its `AskUserQuestion`, answered as the CLI took it live: the input
        // with `answers` by question text.
        let (lines, _) = asking_helper(ID);
        let question = captured(include_str!("testdata/question-2.1.325.jsonl"));
        for v in &question[..2] {
            read_line(ID, v);
        }
        let a = ask(ID).expect("held");
        assert_eq!(a.kind, "question");
        answer(ID, a.id.as_deref(), &["Red".to_string()], None, None, |_| Ok(())).unwrap();
        let (to, said) = written(&lines);
        assert_eq!(to, question[1]["request_id"].as_str().unwrap());
        assert_eq!(said, question[2]["response"]["response"], "what the CLI took");
        lock().entries.remove(ID);
    }

    /// What is refused at once, never shown to the caller: an ask about no tool
    /// call this turn showed (a deny written), the same request again (one
    /// held), one past `ASKS_PER_TURN` (a deny written), and any with its stdin
    /// closed (nothing written: its CLI has failed it). Held asks go with
    /// their stdin, with their process, or after `ASK_TIMEOUT` (a deny).
    #[test]
    fn asks_on_stdout_are_bounded_and_bound_to_their_turn() {
        const ID: &str = "stdio-bounds";
        let call = |n: usize| {
            json!({ "type": "assistant", "parent_tool_use_id": null,
                "message": { "content": [{ "type": "tool_use", "id": format!("toolu_{n}"), "name": "Bash", "input": { "command": "ls" } }] } })
        };
        let asked = |n: usize, request: &str| {
            json!({ "type": "control_request", "request_id": request,
                "request": { "subtype": "can_use_tool", "tool_name": "Bash", "input": { "command": "ls" }, "tool_use_id": format!("toolu_{n}") } })
        };
        let held = || lock().entries[ID].held.len();
        let (lines, _) = asking_helper(ID);

        read_line(ID, &asked(0, "r-forged"));
        assert_eq!(held(), 0, "no such call in this turn");
        let (to, said) = written(&lines);
        assert_eq!((to.as_str(), said["behavior"].as_str()), ("r-forged", Some("deny")));

        for n in 0..ASKS_PER_TURN {
            read_line(ID, &call(n));
            read_line(ID, &asked(n, &format!("r-{n}")));
        }
        read_line(ID, &asked(0, "r-0"));
        assert_eq!(held(), ASKS_PER_TURN, "the same request held once");
        assert!(lines.try_recv().is_err());
        read_line(ID, &call(99));
        read_line(ID, &asked(99, "r-99"));
        assert_eq!(held(), ASKS_PER_TURN, "one past the cap");
        assert_eq!(written(&lines).0, "r-99");

        // Past ASK_TIMEOUT: a deny for each, and they go.
        {
            let mut r = lock();
            let late = std::time::Instant::now().checked_sub(ASK_TIMEOUT).unwrap();
            for h in r.entries.get_mut(ID).unwrap().held.iter_mut().take(2) {
                h.reply.as_mut().unwrap().since = late;
            }
        }
        expire_asks();
        assert_eq!(held(), ASKS_PER_TURN - 2);
        assert_eq!([written(&lines).0, written(&lines).0], ["r-0", "r-1"]);

        // Its stdin closed: what it held goes, unanswerable, and an answer to
        // it is refused; a new ask is taken for nothing, and nothing is written.
        let open = ask(ID).unwrap().id;
        lock().entries.get_mut(ID).unwrap().run.as_mut().unwrap().feed = None;
        assert!(answer(ID, open.as_deref(), &[], Some(true), None, |_| Ok(())).is_err());
        assert_eq!(held(), 0);
        read_line(ID, &call(7));
        read_line(ID, &asked(7, "r-late"));
        assert_eq!(held(), 0, "after its stdin closed");
        assert!(lines.try_recv().is_err());

        // An ask held for one process is not the next one's.
        let (_lines, _) = asking_helper(ID);
        read_line(ID, &call(1));
        read_line(ID, &asked(1, "r-old"));
        assert_eq!(held(), 1);
        let _next = feeding(lock().entries.get_mut(ID).unwrap());
        expire_asks();
        assert_eq!(held(), 0, "its process gone");
        lock().entries.remove(ID);
    }

    /// An ask of the CLI's own machinery is about no call of the model's: one
    /// naming no tool call, and the sandbox's network ask under an id it makes
    /// up (shaped as 2.1.325's bundle writes it: `sendRequest`, `can_use_tool`,
    /// `tool_name` `SandboxNetworkAccess`, `input` `{host}`), are held and
    /// routed. An ask it withdraws (`control_cancel_request`) goes at once,
    /// its place freed, and a late answer to it is refused.
    #[test]
    fn machinery_asks_are_held_and_withdrawn_asks_go() {
        const ID: &str = "stdio-machinery";
        let (lines, _) = asking_helper(ID);
        let network = json!({ "type": "control_request", "request_id": "r-net", "request": {
            "subtype": "can_use_tool", "tool_name": "SandboxNetworkAccess", "display_name": "SandboxNetworkAccess",
            "input": { "host": "registry.npmjs.org" }, "tool_use_id": "6f1c2b9e-0d7a-4c55-9a51-3b1f0e6a2c44",
            "description": "Allow network connection to registry.npmjs.org?" } });
        let unnamed = json!({ "type": "control_request", "request_id": "r-none", "request": {
            "subtype": "can_use_tool", "tool_name": "Bash", "input": { "command": "ls" } } });
        read_line(ID, &network);
        read_line(ID, &unnamed);
        assert!(lines.try_recv().is_err(), "neither refused");
        let held: Vec<Option<String>> = lock().entries[ID].held.iter().map(|h| h.ask.tool.clone()).collect();
        assert_eq!(held, [Some("SandboxNetworkAccess".to_string()), Some("Bash".to_string())]);

        let first = ask(ID).unwrap().id;
        read_line(ID, &json!({ "type": "control_cancel_request", "request_id": "r-net" }));
        assert_eq!(lock().entries[ID].held.len(), 1, "withdrawn, it goes");
        assert!(answer(ID, first.as_deref(), &[], Some(true), None, |_| Ok(())).is_err(), "too late");
        assert!(lines.try_recv().is_err(), "nothing written for it");
        let (to, said) = {
            let a = ask(ID).unwrap();
            answer(ID, a.id.as_deref(), &[], Some(true), None, |_| Ok(())).unwrap();
            written(&lines)
        };
        assert_eq!((to.as_str(), said["behavior"].as_str()), ("r-none", Some("allow")));
        lock().entries.remove(ID);
    }
}
