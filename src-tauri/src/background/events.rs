//! What a turn's CLI says on its stream, Claude Code's and Codex's, and what is
//! made of a turn once it is over.

use super::*;

/// Codex's tools have this whatever its config keeps from them (`procs::also_marked`).
pub(super) fn mark_by_thread(e: &Entry) {
    if let (Provider::Codex, Some(job), Some(thread)) = (e.spec.provider, e.job.as_deref(), e.session.as_deref()) {
        crate::procs::also_marked(job, "CODEX_THREAD_ID", thread);
    }
}

/// Kills a turn's process that is not to run, its tree with it, and reaps it.
pub(super) fn abandon(mut child: std::process::Child, pid: Option<u32>) {
    crate::pty::kill_tree(pid);
    let _ = child.kill();
    let _ = child.wait();
}

/// Claude Code has taken the line `uuid` in: what it carried is the model's now,
/// wherever it was kept. Its word can come after a stop has put the text by.
/// Whether it carried anything.
fn took(e: &mut Entry, uuid: &str) -> bool {
    let mut landed = Vec::new();
    for store in [&mut e.in_flight, &mut e.fed, &mut e.undelivered] {
        let (of_line, rest): (Vec<Queued>, Vec<Queued>) = std::mem::take(store).into_iter().partition(|q| q.line.as_deref() == Some(uuid));
        *store = rest;
        landed.extend(of_line);
    }
    e.budgets_landed.extend(landed.iter().flat_map(|q| q.budget.iter().map(|m| m.id.clone())));
    for q in &landed {
        note(e, &format!("took in: {}", one_line(&q.text)));
    }
    !landed.is_empty()
}

/// A background helper has nobody to answer the CLI's own requests on stdin (an
/// MCP server's elicitation comes this way with stdin open), which would hold
/// its turn for good. An error reads as the cancel it gets with stdin closed.
fn refuse_request(e: &mut Entry, v: &Value) {
    let Some(request) = v["request_id"].as_str() else { return };
    let line = json!({
        "type": "control_response",
        "response": { "subtype": "error", "request_id": request, "error": "a background helper has nobody to ask" },
    });
    let Some(run) = e.run.as_ref() else { return };
    let refused = run.feed.as_ref().is_some_and(|feed| feed.send(format!("{line}\n")).is_ok());
    if refused {
        note(e, &format!("refused the CLI's request: {}", v["request"]["subtype"].as_str().unwrap_or("?")));
    }
}

/// What an event of a Claude Code turn says of the helper; whether a message of
/// its has landed, which is to be saved.
pub(super) fn on_claude_event(e: &mut Entry, v: &Value) -> bool {
    match (v["type"].as_str(), v["state"].as_str()) {
        (Some("user"), _) if v["isReplay"] == true => return v["uuid"].as_str().is_some_and(|u| took(e, u)),
        (Some("command_lifecycle"), Some("started")) => return v["command_uuid"].as_str().is_some_and(|u| took(e, u)),
        // A line the CLI will not run gets no `result`: one its transcript has
        // already is passed over, and a stdin left open would hold it for good.
        // lang-check: allow
        (Some("command_lifecycle"), Some("completed" | "cancelled")) => {
            if let Some(run) = e.run.as_mut().filter(|run| run.line.is_some() && run.line.as_deref() == v["command_uuid"].as_str()) {
                run.feed = None;
            }
            return false;
        }
        (Some("control_request"), _) => {
            refuse_request(e, v);
            return false;
        }
        _ => {}
    }
    if v["type"] == "assistant" {
        // The CLI's own stand-in for a message no model wrote reads `<synthetic>`.
        if v["message"]["model"].as_str().is_some_and(|m| !m.starts_with('<')) {
            e.observed.acted = true;
        }
    }
    match (v["type"].as_str(), v["subtype"].as_str()) {
        (Some("system"), Some("init")) => {
            e.session = v["session_id"].as_str().map(str::to_owned).or(e.session.take());
            // Where its first turn started, or the worktree it made: the init of a
            // turn the CLI takes up by itself (a background task's end) says where
            // its shell has since gone with `cd`, which is not where it works.
            if e.cwd.is_none() {
                e.cwd = v["cwd"].as_str().map(str::to_owned);
            }
            // Its `model` is only what it was asked for, resolved: a model that
            // does not exist reads there too, and the turn fails on it.
        }
        (Some("assistant"), _) if v["parent_tool_use_id"].is_null() => {
            // The model that wrote it: a fallback mid-turn shows here too.
            if let Some(m) = v["message"]["model"].as_str().filter(|m| !m.starts_with('<')) {
                e.observed.model = Some(m.to_string());
            }
            for b in v["message"]["content"].as_array().into_iter().flatten() {
                match b["type"].as_str() {
                    Some("text") => note(e, &format!("said: {}", one_line(b["text"].as_str().unwrap_or("")))),
                    Some("tool_use") => {
                        let a = Ask::from_claude_tool(b["name"].as_str().unwrap_or("?"), &b["input"]);
                        note(e, &format!("{}: {}", a.tool.unwrap_or_default(), one_line(&a.detail.unwrap_or_default())));
                    }
                    _ => {}
                }
            }
        }
        (Some("result"), sub) => {
            let ok = sub == Some("success") && v["is_error"].as_bool() != Some(true);
            let secs = v["duration_ms"].as_f64().unwrap_or(0.0) / 1000.0;
            // From here on text waits for the next process: stdin's end lets
            // this one finish what it has and go, as it would have with stdin
            // closed all along. What it has been given it still runs.
            let earlier_failed = e.run.as_mut().is_some_and(|run| {
                run.feed = None;
                std::mem::replace(&mut run.result_failed, !ok)
            });
            if ok {
                note(e, &format!("turn done in {secs:.0}s"));
                if earlier_failed {
                    e.error = None;
                }
            } else {
                let why = v["result"].as_str().or(sub).unwrap_or("error").to_string();
                note(e, &format!("turn failed: {}", one_line(&why)));
                e.error = Some(why);
            }
        }
        _ => {}
    }
    false
}

/// The model Codex says it moved a turn to, from the one error item that is no
/// error: `model rerouted: <from> -> <to> (<reason>)`.
fn rerouted_to(message: &str) -> Option<&str> {
    let (_, moved) = message.split_once("model rerouted: ")?;
    let (_, to) = moved.split_once(" -> ")?;
    let to = to.split(" (").next()?.trim();
    (!to.is_empty()).then_some(to)
}

pub(super) fn on_codex_event(e: &mut Entry, v: &Value) {
    let item = &v["item"];
    if v["type"].as_str().is_some_and(|t| t.starts_with("item.")) && item["type"] != "error" {
        e.observed.acted = true;
    }
    match v["type"].as_str() {
        Some("thread.started") => {
            e.session = v["thread_id"].as_str().map(str::to_owned).or(e.session.take());
            mark_by_thread(e);
        }
        Some("item.completed") => match item["type"].as_str() {
            Some("agent_message") => note(e, &format!("said: {}", one_line(item["text"].as_str().unwrap_or("")))),
            Some("command_execution") => note(
                e,
                &format!(
                    "ran: {} → {}",
                    one_line(item["command"].as_str().unwrap_or("")),
                    item["exit_code"].as_i64().map(|c| c.to_string()).unwrap_or_else(|| "?".into())
                ),
            ),
            Some("error") if item["message"].as_str().and_then(rerouted_to).is_some() => {
                let message = item["message"].as_str().unwrap_or("");
                e.observed.model = rerouted_to(message).map(str::to_owned);
                e.observed.rerouted = true;
                note(e, &one_line(message));
            }
            Some(other) if other != "reasoning" => note(e, other),
            _ => {}
        },
        Some("turn.failed") | Some("error") => {
            let why = v["error"]["message"].as_str().or(v["message"].as_str()).unwrap_or("error").to_string();
            note(e, &format!("error: {}", one_line(&why)));
            // Codex says its retries the same way ("Reconnecting... 2/5"): a
            // turn that gets through after them has not failed.
            if !why.starts_with("Reconnecting") {
                e.error = Some(why);
            }
        }
        Some("turn.completed") => {
            note(e, "turn done");
            e.error = None;
        }
        _ => {}
    }
}

/// The process of a Claude Code turn has gone and all it said has been read:
/// what it never took in, it never will. What joined its turn gets one more try,
/// as the start of the next and ahead of what was queued behind it, which came
/// later; returned, for its senders. A batch it was started on and never took
/// goes back to who sent it, with anything that joined behind it: a CLI that went
/// before taking even that would most likely do the same again.
pub(super) fn sort_out_untaken(e: &mut Entry) -> Vec<Queued> {
    if !e.in_flight.is_empty() {
        let batch = std::mem::take(&mut e.in_flight);
        e.undelivered.extend(batch);
        let fed = std::mem::take(&mut e.fed);
        e.undelivered.extend(fed);
        note(e, "its CLI went before taking in what it was started on");
        return Vec::new();
    }
    let retry = std::mem::take(&mut e.fed);
    if !retry.is_empty() {
        note(e, "its CLI went before taking in what joined the turn: it starts the next");
        let behind = std::mem::take(&mut e.queue);
        e.queue = [retry.clone(), behind].concat();
    }
    retry
}

/// The turn's process has exited: count the turn, let any held question go,
/// and start the turn the queue has waiting.
pub(super) fn turn_over(id: &str, code: Option<i32>) {
    let (job, strays) = match lock().entries.get(id) {
        Some(e) => (e.job.clone(), e.run.as_ref().filter(|_| !e.observed.acted).and_then(|run| Some((run.pid?, run.started_ms)))),
        None => (None, None),
    };
    // A CLI that went before its model did anything (an unknown `--agent`, a
    // model that is not there) leaves what it started of its own, a `git` it
    // asked where it was: no job of anyone's, and no reason to call the helper
    // busy or keep it from going. Only what is its own by descent: what earlier
    // turns left, and what they started since, stay.
    if let (Some(job), Some((cli, since))) = (&job, strays) {
        let ended = crate::procs::end_tree_of(job, cli, since);
        if !ended.is_empty() {
            crate::log::info(
                "background",
                &format!(
                    "{}: its CLI went before its model did anything; ended what it had started: {}",
                    crate::agents::label(id),
                    ended.join(", ")
                ),
            );
        }
    }
    // Before the turn reads as over: whoever is owed its answer must not hear
    // "finished" ahead of the job it left running.
    let left = job.map(|job| crate::procs::left_in(&job)).unwrap_or_default();
    let (next, thread, generation, rerouted, acted, retried, count) = {
        let mut r = lock();
        let Some(e) = r.entries.get_mut(id) else { return };
        e.run = None;
        e.turns_ended += 1;
        e.observed.turn_ended_ms = crate::throttle::now_ms();
        e.held.clear();
        e.left_running = !left.is_empty();
        // What was already there before the turn keeps its own clock; what is
        // new is as quiet as the turn's end.
        let now = crate::throttle::now_ms();
        e.left = leftovers(&e.left, left, now, now);
        if code != Some(0) && e.error.is_none() {
            e.error = Some(match code {
                Some(c) => format!("{} exited with code {c}", e.spec.provider.as_str()),
                // A signal on Unix: nothing says which but that it was not its own doing.
                None => format!("{} was killed", e.spec.provider.as_str()),
            });
        }
        // Codex says where it works only in its rollout.
        if e.spec.provider == Provider::Codex && e.cwd.is_none() {
            if let Some(thread) = &e.session {
                e.cwd = crate::codex::session::thread_cwd(&e.spec.account_path, thread);
            }
        }
        let retried = match e.spec.provider {
            Provider::Claude => sort_out_untaken(e),
            Provider::Codex => {
                e.in_flight.clear();
                Vec::new()
            }
        };
        let next = (!e.queue.is_empty()).then(|| {
            claim(e);
            reserve(e)
        });
        let thread = e.session.clone().filter(|_| e.spec.provider == Provider::Codex).map(|t| (e.spec.account_path.clone(), t));
        e.observed.answered |= e.observed.acted;
        (next, thread, e.observed.generation, std::mem::take(&mut e.observed.rerouted), e.observed.acted, retried, e.turns_ended)
    };
    // Ahead of anything that could be read as the answer: the helper reads as
    // busy from the claim above until this has moved what its senders wait on.
    let senders: Vec<String> = retried.iter().filter_map(|q| q.from.clone()).collect();
    crate::agents::retried(id, &senders, count);
    reg().1.notify_all();
    crate::log::info("background", &format!("{} turn over, code {code:?}", crate::agents::label(id)));
    // Codex's events name no model but a reroute's, which stands for its turn;
    // otherwise its rollout says what the turn ran on, read with the lock let go.
    // Not of a turn the model never got to: the rollout names the model it was
    // to run on before asking for it, one that is not there too.
    let thread = thread.filter(|_| acted && !rerouted);
    if let Some(rollout) = thread.and_then(|(account, t)| crate::codex::session::rollout_of_thread(&account, &t)) {
        let model = crate::codex::session::last_model(&rollout);
        if let Some(e) = lock().entries.get_mut(id) {
            e.observed.apply_rollout(generation, model);
        }
    }
    // One that cannot start is recorded as ended by `run_turn`. Stopped for
    // the core's sake as it started, its text is kept as if still queued.
    if let Some(Batch { text, .. }) = next.and_then(|res| settle_batch(id, res)) {
        // On disk before the CLI has it (`Entry::in_flight`).
        save();
        if run_turn(id, &text).is_err() {
            if let Some(e) = lock().entries.get_mut(id) {
                let batch = std::mem::take(&mut e.in_flight);
                if e.cut_short || !retried.is_empty() {
                    e.undelivered.extend(batch);
                }
            }
        }
    }
    changed();
}

pub(super) fn note(e: &mut Entry, line: &str) {
    if e.log.len() >= LOG_LINES {
        e.log.pop_front();
    }
    e.log.push_back(line.to_string());
}

pub fn one_line(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= LOG_CHARS {
        return flat;
    }
    format!("{}…", flat.chars().take(LOG_CHARS).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reroute_names_the_model_of_its_turn_and_the_rollout_the_next() {
        assert_eq!(rerouted_to("model rerouted: gpt-a -> gpt-b (high risk)"), Some("gpt-b"));
        assert_eq!(rerouted_to("model rerouted: gpt-a -> gpt-b"), Some("gpt-b"));
        assert_eq!(rerouted_to("the model is overloaded"), None);

        let dir = std::env::temp_dir().join(format!("luna-reroute-{}", std::process::id()));
        let day = dir.join("sessions").join("2026").join("11").join("01");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(
            day.join("rollout-2026-11-01T10-31-15-thread-reroute.jsonl"),
            format!("{}\n", r#"{"timestamp":"t","type":"turn_context","payload":{"model":"gpt-a"}}"#),
        )
        .unwrap();
        let mut h = entry(Provider::Codex);
        h.spec.account_path = dir.to_string_lossy().into_owned();
        h.spec.model = Some("gpt-asked".into());
        h.session = Some("thread-reroute".into());
        lock().entries.insert("reroute-1".into(), h);
        let model = || lock().entries.get("reroute-1").unwrap().observed.model.clone();

        assert_eq!(signs("reroute-1").unwrap().resolved_model, None, "before its first turn");
        // The turn's process could not reach its model: what the rollout names is
        // only what it was to run on.
        turn_over("reroute-1", Some(1));
        assert_eq!(model(), None);
        assert_eq!(signs("reroute-1").unwrap().resolved_model, None, "a failed turn ran on nothing");
        // Nor once the next one starts (its error cleared), or a core takes it up.
        lock().entries.get_mut("reroute-1").unwrap().error = None;
        assert_eq!(signs("reroute-1").unwrap().resolved_model, None, "no model has answered yet");
        lock().entries.get_mut("reroute-1").unwrap().error = None;
        on_codex_event(
            lock().entries.get_mut("reroute-1").unwrap(),
            &json!({ "type": "item.completed", "item": { "id": "item_0", "type": "agent_message", "text": "ok" } }),
        );
        turn_over("reroute-1", Some(0));
        assert_eq!(model().as_deref(), Some("gpt-a"), "the rollout, with no reroute");
        on_codex_event(
            lock().entries.get_mut("reroute-1").unwrap(),
            &json!({ "type": "item.completed", "item": { "id": "item_1", "type": "error", "message": "model rerouted: gpt-a -> gpt-b (high risk)" } }),
        );
        assert_eq!(model().as_deref(), Some("gpt-b"));
        turn_over("reroute-1", Some(0));
        assert_eq!(model().as_deref(), Some("gpt-b"), "the rollout does not take it back");
        turn_over("reroute-1", Some(0));
        assert_eq!(model().as_deref(), Some("gpt-a"), "the next turn asks again");
        assert!(lock().entries.get("reroute-1").unwrap().error.is_none(), "no failure");

        // A rollout that does not read: what it was asked to run on.
        {
            let mut r = lock();
            let e = r.entries.get_mut("reroute-1").unwrap();
            e.observed.model = None;
        }
        assert_eq!(signs("reroute-1").unwrap().resolved_model.as_deref(), Some("gpt-asked"));
        lock().entries.remove("reroute-1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_model_is_the_one_the_cli_says_it_used() {
        let mut e = entry(Provider::Claude);
        on_claude_event(&mut e, &json!({ "type": "system", "subtype": "init", "session_id": "s", "model": "claude-haiku-0-1" }));
        assert_eq!(e.observed.model, None, "what it was asked for, which may not be there");
        let said = |model: &str, parent: Value| json!({ "type": "assistant", "parent_tool_use_id": parent, "message": { "model": model, "content": [] } });
        on_claude_event(&mut e, &said("<synthetic>", Value::Null));
        assert_eq!(e.observed.model, None, "no model wrote that");
        assert!(!e.observed.acted);
        on_claude_event(&mut e, &said("claude-haiku-5-5", Value::Null));
        assert_eq!(e.observed.model.as_deref(), Some("claude-haiku-5-5"));
        assert!(e.observed.acted);
        on_claude_event(&mut e, &said("claude-sonnet-5-5", json!("toolu_1")));
        assert_eq!(e.observed.model.as_deref(), Some("claude-haiku-5-5"), "a subagent's model is its own");
        on_claude_event(&mut e, &said("claude-sonnet-5-5", Value::Null));
        assert_eq!(e.observed.model.as_deref(), Some("claude-sonnet-5-5"), "a fallback shows");
    }

    /// A turn Claude Code takes up by itself (a background task's end) starts
    /// with an `init` that says where its shell went with `cd`.
    #[test]
    fn where_a_helper_works_does_not_follow_its_shell() {
        // A project and worktree gone from disk, where the shape is all there is.
        let s = |p: &std::path::Path| p.to_string_lossy().into_owned();
        let project = std::env::temp_dir().join(format!("luna-home-of-{}", std::process::id())).join("p");
        let wt = s(&project.join(".claude").join("worktrees").join("joyful-dragonfly"));
        let inside = |dir: &str, parts: &[&str]| s(&parts.iter().fold(std::path::PathBuf::from(dir), |p, part| p.join(part)));
        let mut e = entry(Provider::Claude);
        e.spec.folder = s(&project);
        e.spec.worktree = true;
        let init = |cwd: &str| json!({ "type": "system", "subtype": "init", "session_id": "s", "cwd": cwd });
        on_claude_event(&mut e, &init(&wt));
        on_claude_event(&mut e, &init(&inside(&wt, &["src-tauri", "src"])));
        assert_eq!(e.cwd.as_deref(), Some(wt.as_str()));
        // What an older core saved with the shell's move in it.
        assert_eq!(home_of(&e.spec, inside(&wt, &["src-tauri", "src"])), wt);
        assert_eq!(home_of(&e.spec, wt.clone()), wt);
        let mut plain = entry(Provider::Claude);
        plain.spec.folder = s(&project);
        assert_eq!(home_of(&plain.spec, inside(&plain.spec.folder, &["src"])), plain.spec.folder, "the folder it was sent to");
        let elsewhere = s(&std::env::temp_dir().join("luna-elsewhere"));
        assert_eq!(home_of(&plain.spec, elsewhere.clone()), elsewhere, "nothing to go by");
        // Sent into a folder of a worktree, that folder is where it works.
        plain.spec.folder = inside(&wt, &["docs"]);
        assert_eq!(home_of(&plain.spec, inside(&wt, &["docs", "img"])), plain.spec.folder);
    }

    #[test]
    fn a_helper_started_afresh_has_started_once_its_cli_names_a_conversation() {
        take_up_for_test("afresh", "C:\\p", 0, &[], Some("the prompt"));
        assert!(never_started("afresh"));
        on_claude_event(
            lock().entries.get_mut("afresh").unwrap(),
            &json!({ "type": "system", "subtype": "init", "session_id": "started-afresh", "cwd": "C:\\p" }),
        );
        assert!(!never_started("afresh"));
        lock().entries.remove("afresh");
    }

    #[test]
    fn a_failed_turn_says_why() {
        let mut e = entry(Provider::Claude);
        on_claude_event(
            &mut e,
            &json!({ "type": "result", "subtype": "error_during_execution", "is_error": true, "result": "rate limited" }),
        );
        assert_eq!(e.error.as_deref(), Some("rate limited"));
        let mut e = entry(Provider::Codex);
        on_codex_event(&mut e, &json!({ "type": "turn.failed", "error": { "message": "boom" } }));
        assert_eq!(e.error.as_deref(), Some("boom"));
        assert!(e.log.back().unwrap().contains("boom"));
    }

    #[test]
    fn a_retry_codex_got_through_is_no_failure() {
        // As `codex exec --json` 0.166 words them on a connection it retries.
        let retry = json!({ "type": "error", "message": "Reconnecting... 2/5 (unexpected status 401 Unauthorized)" });
        let mut e = entry(Provider::Codex);
        on_codex_event(&mut e, &retry);
        assert_eq!(e.error, None, "said in its log, and not a failure");
        assert!(e.log.back().unwrap().contains("Reconnecting"));
        on_codex_event(&mut e, &json!({ "type": "turn.completed" }));
        assert_eq!(e.error, None);
        // One that does not read as a retry, followed by a turn that completes.
        on_codex_event(&mut e, &json!({ "type": "error", "message": "stream hiccup" }));
        assert_eq!(e.error.as_deref(), Some("stream hiccup"));
        on_codex_event(&mut e, &json!({ "type": "turn.completed" }));
        assert_eq!(e.error, None, "the turn got through after all");
        // The retries run out.
        on_codex_event(&mut e, &retry);
        on_codex_event(&mut e, &json!({ "type": "error", "message": "unexpected status 401 Unauthorized" }));
        on_codex_event(&mut e, &json!({ "type": "turn.failed", "error": { "message": "unexpected status 401 Unauthorized" } }));
        assert_eq!(e.error.as_deref(), Some("unexpected status 401 Unauthorized"));
    }

    #[test]
    fn its_first_result_ends_stdin_and_text_then_waits_for_the_next_process() {
        let mut e = entry(Provider::Claude);
        let lines = feeding(&mut e);
        on_claude_event(&mut e, &json!({ "type": "result", "subtype": "success", "duration_ms": 10 }));
        assert!(matches!(lines.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected)), "the end of its input");
        assert!(e.run.is_some(), "the turn is over when its process is");
        assert_eq!(place(&mut e, "too late for it".into(), true), Placed::Queued);
    }

    /// A line the CLI passes over has no `result`, which must not hold stdin open.
    #[test]
    fn the_end_of_the_line_it_started_on_ends_stdin_and_of_another_does_not() {
        let mut e = entry(Provider::Claude);
        let lines = feeding(&mut e);
        let ended = |uuid: &str, state: &str| json!({ "type": "command_lifecycle", "command_uuid": uuid, "state": state });
        on_claude_event(&mut e, &ended("a-joined-line", "completed"));
        assert!(!matches!(lines.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected)));
        on_claude_event(&mut e, &ended("opening-line", "queued"));
        assert!(e.run.as_ref().unwrap().feed.is_some());
        on_claude_event(&mut e, &ended("opening-line", "completed"));
        assert!(matches!(lines.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected)));

        let mut e = entry(Provider::Claude);
        let _lines = feeding(&mut e);
        // lang-check: allow
        on_claude_event(&mut e, &ended("opening-line", "cancelled"));
        assert!(e.run.as_ref().unwrap().feed.is_none());
    }

    #[test]
    fn the_clis_word_is_what_retires_a_message_not_the_pipe() {
        let mut e = entry(Provider::Claude);
        let _lines = feeding(&mut e);
        assert_eq!(place(&mut e, "joined".into(), true), Placed::Joined);
        let uuid = e.fed[0].id.clone();
        assert!(!on_claude_event(&mut e, &json!({ "type": "user", "uuid": uuid, "message": {} })), "a tool result, say");
        assert!(
            !on_claude_event(&mut e, &json!({ "type": "command_lifecycle", "command_uuid": uuid, "state": "queued" })),
            "queued is lost with a kill"
        );
        assert_eq!(e.fed.len(), 1);
        assert!(!on_claude_event(&mut e, &json!({ "type": "user", "isReplay": true, "uuid": "another" })));
        assert!(on_claude_event(&mut e, &json!({ "type": "user", "isReplay": true, "uuid": uuid })));
        assert!(e.fed.is_empty());

        // A batch goes as the line of its first, the opening one too.
        let mark = BudgetMark { id: "kid".into(), line: "over budget".into() };
        e.in_flight = vec![Queued::new(None, "one".into(), vec![mark]), "two".into()];
        stamp(&mut e.in_flight);
        let uuid = e.in_flight[0].id.clone();
        assert!(on_claude_event(&mut e, &json!({ "type": "command_lifecycle", "command_uuid": uuid, "state": "started" })));
        assert!(e.in_flight.is_empty());
        assert_eq!(e.budgets_landed, ["kid"], "a budget line counts once the CLI has it");
    }

    #[test]
    fn the_clis_own_requests_are_refused_on_stdin_rather_than_left_to_hang() {
        let mut e = entry(Provider::Claude);
        let lines = feeding(&mut e);
        let asked = json!({ "type": "control_request", "request_id": "r-1", "request": { "subtype": "elicitation", "message": "Pick" } });
        assert!(!on_claude_event(&mut e, &asked));
        let v: Value = serde_json::from_str(&lines.try_recv().unwrap()).unwrap();
        assert_eq!(v["type"].as_str(), Some("control_response"));
        assert_eq!((v["response"]["subtype"].as_str(), v["response"]["request_id"].as_str()), (Some("error"), Some("r-1")));
        // Its stdin has ended: the CLI cancels that by itself.
        e.run.as_mut().unwrap().feed = None;
        on_claude_event(&mut e, &asked);
        assert!(lines.try_recv().is_err());
    }

    #[test]
    fn a_later_result_clears_what_an_earlier_one_of_the_process_left_and_nothing_else() {
        let mut e = entry(Provider::Claude);
        let failed = json!({ "type": "result", "subtype": "error_during_execution", "is_error": true, "result": "rate limited" });
        let done = json!({ "type": "result", "subtype": "success", "duration_ms": 1 });
        e.run = Some(Run::claimed(None, 0));
        on_claude_event(&mut e, &failed);
        assert_eq!(e.error.as_deref(), Some("rate limited"));
        on_claude_event(&mut e, &done);
        assert_eq!(e.error, None, "what was fed in behind it went through");

        e.error = Some("it was stopped, by luna_kill or from the window".into());
        on_claude_event(&mut e, &done);
        assert!(e.error.is_some(), "not a result's to clear");
    }
}
