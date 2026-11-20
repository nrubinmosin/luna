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
pub(super) fn took(e: &mut Entry, uuid: &str) -> bool {
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

/// A permission prompt or `AskUserQuestion` its CLI asks on its stdout
/// (`--permission-prompt-tool stdio`, 2.1.325): `{"type":"control_request",
/// "request_id", "request":{"subtype":"can_use_tool", "tool_name", "input",
/// "tool_use_id", ...}}`. Its id and request, held for the caller and
/// answered on stdin (ask.rs `take_ask`).
pub(super) fn permission_request(v: &Value) -> Option<(String, Value)> {
    (v["type"] == "control_request" && v["request"]["subtype"] == "can_use_tool")
        .then(|| Some((v["request_id"].as_str()?.to_string(), v["request"].clone())))
        .flatten()
}

/// How many of a process's tool call ids are kept for its asks (`Run::tool_uses`):
/// more than a turn's subagents have open at once by far.
const TOOL_USES_KEPT: usize = 1024;

/// Whether a background task its CLI lists (`background_tasks_changed`,
/// 2.1.325) may ask for a permission or put a question: a subagent or a
/// workflow (`task_type` `local_agent`, `local_workflow`), or one that says
/// no type. A shell (`local_bash`) or a monitor asks nothing.
fn may_ask(task: &Value) -> bool {
    matches!(task["task_type"].as_str(), None | Some("local_agent" | "local_workflow"))
}

/// A background helper has nobody to answer the CLI's other requests on stdin
/// (an MCP server's elicitation comes this way with stdin open), which would
/// hold its turn for good. An error reads as the cancel it gets with stdin
/// closed.
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

/// Who made the tool calls of an assistant message: the helper itself, or a
/// subagent of it, whose messages carry `parent_tool_use_id` and (2.1.325)
/// `agent_id` and `subagent_type`. Kept for a permission call that names the
/// call's id (`permission`), and said of one already held, the message having
/// come in after it.
fn made_calls(e: &mut Entry, v: &Value) {
    let by = if v["parent_tool_use_id"].is_null() {
        crate::agents::AskSource::session()
    } else {
        crate::agents::AskSource::subagent(v["agent_id"].as_str(), v["subagent_type"].as_str())
    };
    let calls = v["message"]["content"].as_array().into_iter().flatten().filter(|b| b["type"] == "tool_use");
    for call in calls.filter_map(|b| b["id"].as_str()) {
        for h in e.held.iter_mut().filter(|h| h.ask.source.is_none() && h.tool_use_id.as_deref() == Some(call)) {
            h.ask.source = Some(by.clone());
        }
        if e.calls.len() == CALLS {
            e.calls.pop_front();
        }
        e.calls.push_back((call.to_string(), by.clone()));
        // For its asks on stdout to be about (ask.rs `take_ask`).
        if let Some(run) = e.run.as_mut() {
            if run.tool_uses.len() == TOOL_USES_KEPT {
                run.tool_uses.pop_front();
            }
            run.tool_uses.push_back(call.to_string());
        }
    }
}

/// How long a lead's stdin stays open past its turn's `result` at most
/// (`keeps_listening`). The CLI's own wait for its background work
/// (command.rs `BG_WAIT_CEILING_MS`) holds only once stdin is closed, and
/// counts from there (2.1.325): this bounds the stretch before.
pub(super) const LISTEN_MS: u64 = 10_800_000;

/// Whether a helper's stdin stays open past a `result`: while its CLI says it
/// has a background subagent or workflow running (`may_ask`), whose asks come
/// over it (ask.rs `take_ask`; one closed under an ask fails it), and for
/// LISTEN_MS at most. Its shells and monitors alone do not keep it: they ask
/// nothing, and with stdin closed the CLI's own wind-down ends them. A lead's
/// stays open while any background task runs (subagents, shells, monitors),
/// which keep `claude -p` up after the result in any case, for the notices
/// its helpers owe it to go in as stream-json lines (queue.rs `send_as`). Claude Code 2.1.325 takes
/// such a line at once and answers it with a turn and a `result` of its own,
/// the background work running on. With stdin open it never goes by itself,
/// so stdin is closed once the last task ends, at a `result` with none left,
/// or, for a lead, when nothing is owed to it any more
/// (`listen_only_while_owed`): only a lead has Luna helpers to owe it. Each of
/// those closes waits for the lines written to it to be taken in
/// (`holds_untaken`); only LISTEN_MS does not. Not once Luna has asked the
/// turn to stop (control.rs `ask_to_stop`).
fn keeps_listening(e: &Entry, now: u64) -> bool {
    e.run.as_ref().is_some_and(|run| {
        (if e.spec.tools { run.background_tasks } else { run.asking_tasks }) > 0
            && run.feed.is_some()
            && run.interrupt.is_none()
            && run.listening_since.is_none_or(|since| now.saturating_sub(since) < LISTEN_MS)
    })
}

/// Whether a stdin kept open for notices (`keeps_listening`) has a line
/// written to it that its CLI is yet to take in, short of LISTEN_MS: then a
/// `result` is of a turn the CLI started by itself, and leaves it open.
fn holds_untaken(e: &Entry, now: u64) -> bool {
    !e.fed.is_empty()
        && e.run
            .as_ref()
            .is_some_and(|run| run.feed.is_some() && run.listening_since.is_some_and(|since| now.saturating_sub(since) < LISTEN_MS))
}

/// Its stdin's end: what it was given it still runs, then goes.
fn stop_listening(run: &mut Run) {
    run.feed = None;
    run.listening_since = None;
}

/// A lead whose stdin is kept open for notices, between turns of its CLI
/// (`keeps_listening`): a notice joins it there rather than wait for its
/// process to go. Not while a line written to it is still to be taken in:
/// the turn that takes it is yet to come, and a `result` before then is of
/// one the CLI started by itself.
pub(super) fn listening(e: &Entry) -> bool {
    e.spec.tools
        && !e.held_back()
        && e.held.is_empty()
        && e.fed.is_empty()
        && e.run.as_ref().is_some_and(|run| run.after_result && run.feed.is_some() && run.listening_since.is_some())
}

/// Whether a background helper whose turn runs takes a notice now (`listening`).
pub fn takes_notices(id: &str) -> bool {
    lock().entries.get(id).is_some_and(listening)
}

/// Closes the stdin kept open for notices (`keeps_listening`) of each lead
/// open for LISTEN_MS, or with every line written to it taken in and, between
/// turns of its CLI, owed nothing now. `owed`: the callers
/// some session owes an answer, read under the registry's lock the caller
/// still holds: a debt that comes after this look finds stdin as it left it.
pub fn listen_only_while_owed(owed: &std::collections::HashSet<String>) {
    let now = crate::throttle::now_ms();
    for (id, e) in lock().entries.iter_mut() {
        listen_while_owed(e, owed.contains(id), now);
    }
}

/// `listen_only_while_owed` for one helper, `owed` an answer or not.
pub(super) fn listen_while_owed(e: &mut Entry, owed: bool, now: u64) {
    // Its CLI's asks wait on this stdin (ask.rs `take_ask`), and one closed
    // under them fails them: not while one is held, nor, for a lead owed
    // nothing, while a background subagent that may ask runs.
    let (untaken, asking, lead) = (!e.fed.is_empty(), !e.held.is_empty(), e.spec.tools);
    let Some(run) = e.run.as_mut() else { return };
    let Some(since) = run.listening_since else { return };
    let why = if now.saturating_sub(since) >= LISTEN_MS {
        format!("it has been open for {} hours", LISTEN_MS / 3_600_000)
    } else if untaken || asking {
        return;
    } else if run.after_result && !owed && lead && run.asking_tasks == 0 {
        "nothing is owed to it now".to_string()
    } else {
        return;
    };
    stop_listening(run);
    // What it held went with its stdin: its CLI has it as failed.
    e.held.clear();
    note(e, &format!("its stdin is closed: {why}"));
}

/// What was queued for a lead whose CLI now takes notices on stdin again
/// (`listening`), in the order it came, while what heads the queue is plain
/// text: a notice that found it busy at delivery, and what was sent behind
/// it, which would otherwise wait for its process to go however long its
/// background tasks run. Text its CLI would read as a command stays for the
/// next process, its sender asked again then (queue.rs `settle_batch`), and
/// what came after it with it. Whether any went. What went is kept in
/// `Entry::drained`: its senders' answers end this process now, not the next
/// one their debts were put past as it queued (agents/owed.rs
/// `moved_onto_stdin`).
pub(super) fn feed_queued(e: &mut Entry) -> bool {
    // Nothing new goes in while the start gate is closed: it stays queued,
    // which an update's stop keeps for its sender (`crate::starts`).
    if !listening(e) || !crate::starts::open_to_all() {
        return false;
    }
    let mut fed = false;
    while e.queue.first().is_some_and(|q| crate::agents::command_prefix(&q.text).is_none()) {
        let q = e.queue.remove(0);
        let moved = Drained { id: q.id.clone(), from: q.from.clone(), turns_ended: e.turns_ended };
        if let Err(q) = write_line(e, q) {
            e.queue.insert(0, q);
            break;
        }
        e.drained.push(moved);
        fed = true;
    }
    fed
}

/// The ids of what `feed_queued` has moved onto stdin past the first `from`
/// of `Entry::drained`: what an event moved, `from` being their count before
/// it. Nothing trims that list while a process runs (`turn_over`).
pub(super) fn drained_since(e: &Entry, from: usize) -> Vec<String> {
    e.drained.iter().skip(from).map(|d| d.id.clone()).collect()
}

/// Of text queued for `id`, by `Queued::id`: whether it went onto the stdin
/// of a process instead (`feed_queued`), the one running now or one gone
/// since. If so, its sender and the count that process ends past, as for a
/// line that joined it; None too while more of that sender's text waits for
/// a process after it: in the queue, or with that one gone, in what a later
/// one was started on or took on its stdin.
pub fn moved_onto_stdin_at(id: &str, qid: &str) -> Option<(Option<String>, u32)> {
    let r = lock();
    let e = r.entries.get(id)?;
    let d = e.drained.iter().find(|d| d.id == qid)?;
    let mine = |q: &Queued| q.from == d.from;
    let gone = e.turns_ended > d.turns_ended;
    let later = e.queue.iter().any(mine)
        || (gone && (e.in_flight.iter().any(mine) || e.fed.iter().any(mine)))
        || e.drained.iter().any(|l| l.from == d.from && l.turns_ended > d.turns_ended);
    (!later).then(|| (d.from.clone(), d.turns_ended))
}

/// What an event of a Claude Code turn says of the helper; whether a message of
/// its has landed, or gone from its queue onto its stdin, which is to be saved.
pub(super) fn on_claude_event(e: &mut Entry, v: &Value) -> bool {
    match (v["type"].as_str(), v["state"].as_str()) {
        (Some("user"), _) if v["isReplay"] == true => return v["uuid"].as_str().is_some_and(|u| took(e, u)),
        (Some("command_lifecycle"), Some("started")) => return v["command_uuid"].as_str().is_some_and(|u| took(e, u)),
        // A line the CLI will not run gets no `result`: one its transcript has
        // already is passed over, and a stdin left open would hold it for good.
        // The end of one that ran comes right behind its `result`, which may
        // have kept stdin open for notices (`keeps_listening`).
        // lang-check: allow
        (Some("command_lifecycle"), Some("completed" | "cancelled")) => {
            if let Some(run) = e
                .run
                .as_mut()
                .filter(|run| run.line.is_some() && run.line.as_deref() == v["command_uuid"].as_str() && run.listening_since.is_none())
            {
                run.feed = None;
            }
            return false;
        }
        // Held for its caller (run.rs `take_event`).
        (Some("control_request"), _) if permission_request(v).is_some() => return false,
        // An ask it withdraws (2.1.325: `{"type":"control_cancel_request",
        // "request_id"}`, when its own wait for it ends): gone at once, its
        // place among the held freed, a late answer to it refused.
        (Some("control_cancel_request"), _) => {
            let claim = e.run.as_ref().map(|run| run.claim);
            let request = v["request_id"].as_str();
            let before = e.held.len();
            e.held.retain(|h| !h.reply.as_ref().is_some_and(|to| Some(to.claim) == claim && Some(to.request_id.as_str()) == request));
            if e.held.len() != before {
                note(e, "its CLI withdrew an ask");
            }
            return false;
        }
        (Some("control_request"), _) => {
            refuse_request(e, v);
            return false;
        }
        // Its word on Luna's interrupt (2.1.325: `{"still_queued":[]}`); the
        // turn's `result` comes behind it. It echoes the answers Luna writes
        // too, which say nothing new.
        (Some("control_response"), _) => {
            let asked = e.run.as_ref().and_then(|run| run.interrupt.as_deref());
            if asked.is_some() && asked == v["response"]["request_id"].as_str() {
                note(e, "its CLI took the interrupt");
            }
            return false;
        }
        _ => {}
    }
    if v["type"] == "assistant" {
        // A stand-in for an API error says which (2.1.325's stream carries the
        // `error` its transcript line has).
        if let (Some(kind), Some(run)) = (v["error"].as_str(), e.run.as_mut()) {
            run.api_error = Some(crate::activity::api_error_kind(kind));
        }
        // The CLI's own stand-in for a message no model wrote reads `<synthetic>`.
        if v["message"]["model"].as_str().is_some_and(|m| !m.starts_with('<')) {
            e.observed.acted = true;
        }
        made_calls(e, v);
    }
    match (v["type"].as_str(), v["subtype"].as_str()) {
        (Some("system"), Some("init")) => {
            e.session = v["session_id"].as_str().map(str::to_owned).or(e.session.take());
            // Where its first turn started, or the worktree it went into: the init of a
            // turn the CLI takes up by itself (a background task's end) says where
            // its shell has since gone with `cd`, which is not where it works.
            if e.cwd.is_none() {
                e.cwd = v["cwd"].as_str().map(str::to_owned);
            }
            // Its `model` is only what it was asked for, resolved: a model that
            // does not exist reads there too, and the turn fails on it.
        }
        // Only one said to run in the foreground is left out: a field gone
        // missing over-reports a loss rather than hide one.
        (Some("system"), Some("task_started")) => {
            if let Some(run) = e.run.as_mut().filter(|_| v["is_backgrounded"] != false) {
                track_subagent(run, v, true);
            }
        }
        (Some("system"), Some("background_tasks_changed")) => {
            let tasks = v["tasks"].as_array();
            let running = tasks.map_or(0, Vec::len);
            let asking = tasks.into_iter().flatten().filter(|t| may_ask(t)).count();
            let (untaken, holding) = (!e.fed.is_empty(), !e.held.is_empty());
            let lead = e.spec.tools;
            let mut closed = false;
            if let Some(run) = e.run.as_mut() {
                run.background_tasks = running;
                run.asking_tasks = asking;
                // A listing with no `tasks` says nothing of which run.
                for task in v["tasks"].as_array().into_iter().flatten() {
                    track_subagent(run, task, false);
                }
                // What kept stdin open is over: the CLI takes a task's end up
                // in a turn of its own and, its stdin closed, goes once that
                // turn and any it is at are done. Not once that turn has
                // begun (its model has written since the `result`,
                // `Run::model_done`): it may ask, and its `result` closes stdin
                // then. A `task_notification` alone keeps nothing open: the
                // CLI may take no turn on it. Nor with a line written to it
                // still to be taken in, or an ask of its held there.
                let kept = if lead { running } else { asking };
                if kept == 0 && run.model_done && !untaken && !holding && run.listening_since.is_some() {
                    stop_listening(run);
                    closed = true;
                }
            }
            if closed {
                note(e, "what kept its stdin open is over: its stdin is closed");
            }
        }
        // A background task's end, which the CLI takes up in a turn of its
        // own (2.1.325): at work from here, ahead of that turn's first
        // `assistant`, so that a notice does not land in it (`listening`).
        (Some("system"), Some("task_notification")) => {
            if let Some(run) = e.run.as_mut() {
                subagent_ended(run, v);
                run.after_result = false;
            }
        }
        (Some("assistant"), _) if v["parent_tool_use_id"].is_null() => {
            // At work on a turn again: one the end of a background job started.
            if let Some(run) = e.run.as_mut() {
                run.after_result = false;
                run.model_done = false;
            }
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
            // closed all along. What it has been given it still runs. But for
            // a lead whose own background tasks keep its CLI up anyway, which
            // takes its helpers' notices on stdin meanwhile (`keeps_listening`),
            // and what was queued for it while it was at work (`feed_queued`).
            // Nor while a line written to that stdin is still to be taken in
            // (`holds_untaken`): this `result` is of a turn the CLI started by
            // itself, and the line's own turn is yet to come.
            let now = crate::throttle::now_ms();
            let keeps = keeps_listening(e, now);
            let untaken = !keeps && holds_untaken(e, now);
            let mut began = false;
            let earlier_failed = e.run.as_mut().is_some_and(|run| {
                run.after_result = true;
                run.model_done = true;
                if keeps {
                    began = run.listening_since.is_none();
                    run.listening_since.get_or_insert(now);
                } else if !untaken {
                    stop_listening(run);
                }
                std::mem::replace(&mut run.result_failed, !ok)
            });
            if began {
                note(e, "its background tasks run on: its stdin stays open for its helpers' notices");
            } else if untaken {
                note(e, "a line written to it is still to be taken in: its stdin stays open");
            }
            // The end of a turn Luna asked its CLI to stop (control.rs
            // `ask_to_stop`; 2.1.325: `error_during_execution`, its
            // `terminal_reason` `aborted_tools`): the stop it is, whose
            // reason the stop has set, not a failure of the turn's.
            let interrupted = !ok && e.run.as_ref().is_some_and(|run| run.interrupt.is_some());
            if ok {
                note(e, &format!("turn done in {secs:.0}s"));
                if earlier_failed && !stopped(e) {
                    e.error = None;
                }
            } else if interrupted || stopped(e) {
                note(e, &format!("turn stopped as asked after {secs:.0}s"));
                e.error.get_or_insert_with(|| INTERRUPTED.into());
            } else {
                let why = v["result"].as_str().or(sub).unwrap_or("error").to_string();
                note(e, &format!("turn failed: {}", one_line(&why)));
                // The API's word where its stream gave one, as a chat's
                // `StopFailure` does: a usage limit is then the record's
                // `usageLimit`, whatever the result's text says.
                let kind = e.run.as_ref().and_then(|run| run.api_error);
                e.error = Some(kind.map_or(why, str::to_owned));
            }
            // Moved from its queue to its stdin: saved as such, so that a core
            // gone meanwhile settles it against the conversation (persist.rs).
            return feed_queued(e);
        }
        _ => {}
    }
    false
}

/// A background subagent its CLI started (`task_started`, `started`), or one
/// moved to the background since, listed by `background_tasks_changed`
/// (2.1.325), by its `task_id`, `run_id` and `description`, one entry per
/// run: a subagent resumed in the same process keeps its `task_id` under a
/// new `run_id`. Its other tasks (shells, monitors, workflows) are not
/// subagents. Nor is one a subagent spawned (`spawn_depth` past 1, a
/// `parent_task_id`): it lives inside its parent, which is the one named. A
/// listing adds only a `task_id` not seen yet, and never touches a run
/// already kept: one late from an earlier run must not stand for a later.
/// Its being left out of a later listing says nothing: only its own
/// `task_notification` ends it (`subagent_ended`).
fn track_subagent(run: &mut Run, task: &Value, started: bool) {
    let (Some(id), Some("local_agent")) = (task["task_id"].as_str(), task["task_type"].as_str()) else { return };
    if task["spawn_depth"].as_u64().is_some_and(|d| d > 1) || task["parent_task_id"].is_string() {
        return;
    }
    let run_id = task["run_id"].as_str().map(str::to_owned);
    let known = |s: &Subagent| s.task_id == id && (!started || s.run_id == run_id);
    if run.subagents.iter().any(known) {
        return;
    }
    let description = task["description"].as_str().map(one_line).filter(|d| !d.is_empty()).unwrap_or_else(|| id.to_string());
    run.subagents.push(Subagent { task_id: id.to_string(), run_id, description, done: false, cut: false });
}

/// A task's end (`task_notification`), for the run it names; one that names
/// none, for the one run of that `task_id` still open, if only one is.
/// `completed` or `failed` is the subagent's own; `stopped` while the main
/// thread is at work, the model's (its `TaskStop`); `stopped` with nothing
/// from the model since the turn's `result`, the CLI's on its way out, which
/// leaves it `cut` for good. Any other status, or none, ends nothing.
fn subagent_ended(run: &mut Run, v: &Value) {
    let model_done = run.model_done;
    let Some(id) = v["task_id"].as_str() else { return };
    let mut runs = run.subagents.iter_mut().filter(|s| s.task_id == id);
    let s = match v["run_id"].as_str() {
        Some(r) => runs.find(|s| s.run_id.as_deref() == Some(r)),
        None => {
            let mut open = runs.filter(|s| !s.done && !s.cut);
            match (open.next(), open.next()) {
                (Some(s), None) => Some(s),
                _ => None,
            }
        }
    };
    let Some(s) = s else { return };
    match v["status"].as_str() {
        Some("completed" | "failed") => s.done = true,
        Some("stopped") if model_done => s.cut = true,
        Some("stopped") => s.done = true,
        _ => {}
    }
}

/// The descriptions of the background subagents a Claude Code turn's process
/// started and never said were done, or that its CLI cut: what went with it
/// unfinished, once each.
pub(super) fn unfinished_subagents(run: &Run) -> Vec<String> {
    let mut ids: Vec<&str> = Vec::new();
    let mut names = Vec::new();
    for s in run.subagents.iter().filter(|s| s.cut || !s.done) {
        if !ids.contains(&s.task_id.as_str()) {
            ids.push(&s.task_id);
            names.push(s.description.clone());
        }
    }
    names
}

/// The process of a turn has gone and all it said has been read: what it
/// never took in, it never will (Claude Code's word for a line, a Codex
/// server's answer to `turn/start` or a steer). What joined its turn gets one more try,
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

/// Whether what a turn's CLI started of its own is ended once it has gone
/// (`turn_over`): its model did nothing, so nothing it started is work; or it
/// went because Luna asked it to stop the turn (control.rs `ask_to_stop`),
/// whose kill would have ended it all.
pub(super) fn ends_its_tree(e: &Entry, run: &Run) -> bool {
    !e.observed.acted || run.interrupt.is_some()
}

/// Whether Luna has stopped the turn running (a stop, an interrupt) and set
/// why: an end its stream wrote before it went, read after, does not stand
/// for the turn's.
pub(super) fn stopped(e: &Entry) -> bool {
    e.error.as_deref().is_some_and(is_stop) && e.run.as_ref().is_some_and(|run| run.luna_stopped)
}

/// The turn's process has exited: count the turn, let any held question go,
/// and start the turn the queue has waiting.
pub(super) fn turn_over(id: &str, code: Option<i32>) {
    let (job, strays, codex, unfinished, luna_stopped) = match lock().entries.get(id) {
        Some(e) => (
            e.job.clone(),
            e.run.as_ref().filter(|run| ends_its_tree(e, run)).and_then(|run| Some((run.pid?, run.started_ms, run.interrupt.is_some()))),
            e.session
                .clone()
                .filter(|_| e.spec.provider == Provider::Codex)
                .zip(e.run.as_ref())
                .map(|(thread, run)| (e.spec.account_path.clone(), thread, run.started_ms)),
            e.run.as_ref().map(unfinished_subagents).unwrap_or_default(),
            e.run.as_ref().is_some_and(|run| run.luna_stopped),
        ),
        None => (None, None, None, Vec::new(), false),
    };
    // A CLI that went before its model did anything (an unknown `--agent`, a
    // model that is not there) leaves what it started of its own, a `git` it
    // asked where it was: no job of anyone's, and no reason to call the helper
    // busy or keep it from going. Only what is its own by descent: what earlier
    // turns left, and what they started since, stay.
    // So too a CLI that went as Luna asked it to stop its turn (control.rs
    // `ask_to_stop`): what its tools started and left goes, as the kill it
    // stands in for would have taken it.
    if let (Some(job), Some((cli, since, asked))) = (&job, strays) {
        let ended = crate::procs::end_tree_of(job, cli, since);
        if !ended.is_empty() {
            let why = if asked { "its CLI stopped its turn as asked" } else { "its CLI went before its model did anything" };
            crate::log::info(
                "background",
                &format!("{}: {why}; ended what it had started: {}", crate::agents::label(id), ended.join(", ")),
            );
        }
    }
    // Before the turn reads as over: whoever is owed its answer must not hear
    // "finished" ahead of the job it left running.
    let left = job.map_or(Some(Vec::new()), |job| crate::procs::left_in(&job));
    // So too the subagents of a Codex turn whose server went, at the turn's
    // end (its stdin closed) or mid-turn, their work unfinished, with whoever
    // is owed an answer now to hear of it: the registry's lock goes before this one.
    let cut_off = codex
        .map(|(account, thread, since)| crate::codex::session::subagents_cut_at_turn_end(&account, &thread, since))
        .filter(|names| !names.is_empty())
        .map(|names| (names, crate::agents::callers_owed(id)));
    // And the background subagents Claude Code's process took with it, which
    // live inside it: started, and never said to be done (2.1.325).
    let lost = (!unfinished.is_empty()).then(|| crate::agents::callers_owed(id));
    let (next, thread, generation, rerouted, acted, retried, count) = {
        let mut r = lock();
        // Its next turn's place on its line (places.rs), counted with its own
        // given back: what it never took in goes into that turn too.
        let lunas = r.entries.get(id).is_some_and(|e| lunas_own(e.queue.iter().chain(&e.fed)));
        let room = room_for_turn(&r, id, lunas);
        // Its asks went with its turn (`e.held`, below): their ids with them.
        r.asked.retain(|_, to| to != id);
        let Some(e) = r.entries.get_mut(id) else { return };
        e.run = None;
        e.turns_ended += 1;
        // A send whose text this process took may have its debt still to go
        // in. What it never took in is no move any more: it goes again with
        // the next process (`sort_out_untaken`, below), which answers it.
        let untaken = &e.fed;
        e.drained.retain(|d| !untaken.iter().any(|q| q.id == d.id));
        let over = e.drained.len().saturating_sub(DRAINED_KEPT);
        e.drained.drain(..over);
        e.observed.turn_ended_ms = crate::throttle::now_ms();
        let by = if luna_stopped { LostBy::Stopped } else { LostBy::Exited };
        if let Some((names, callers)) = cut_off {
            note(e, &format!("its app-server went with {} subagent(s) of the turn unfinished", names.len()));
            record_loss(&mut e.observed, e.turns_ended, StopCause::TurnEnd, luna_stopped.then_some(by), names, callers);
        }
        if let Some(callers) = lost {
            note(e, &format!("its CLI went with background subagents unfinished: {}", unfinished.join("; ")));
            // Its own word on stderr that it stopped them this turn
            // (`take_stderr`) is the same loss, and keeps its cause; this
            // names them.
            record_loss(&mut e.observed, e.turns_ended, StopCause::Ceiling, Some(by), unfinished, callers);
        }
        e.held.clear();
        note_left(e, left);
        // Luna's stop of this turn, kept past the next turn's start (`stop_of`).
        e.observed.stopped = e.error.clone().filter(|why| is_stop(why)).map(|why| (e.turns_ended, why));
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
        let retried = sort_out_untaken(e);
        // Held for promotion or deletion, queued text waits for the hold to
        // let go (control.rs `go_on`). With no place free on its line, it
        // waits for one (places.rs `wake`).
        let ready = !e.queue.is_empty() && !e.held_back();
        if ready && !room {
            wait_for_place(e);
        }
        let next = (ready && room).then(|| {
            claim(e);
            reserve(e)
        });
        let thread = e.session.clone().filter(|_| e.spec.provider == Provider::Codex).map(|t| (e.spec.account_path.clone(), t));
        e.observed.answered |= e.observed.acted;
        (next, thread, e.observed.generation, std::mem::take(&mut e.observed.rerouted), e.observed.acted, retried, e.turns_ended)
    };
    // A turn shorter than the reads between them was work all the same: a
    // power-off's quiet window starts over.
    crate::activity::touch();
    // Ahead of anything that could be read as the answer: the helper reads as
    // busy from the claim above until this has moved what its senders wait on.
    let senders: Vec<String> = retried.iter().filter_map(|q| q.from.clone()).collect();
    crate::agents::retried(id, &senders, count);
    reg().1.notify_all();
    crate::log::info("background", &format!("{} turn over, code {code:?}", crate::agents::label(id)));
    // A Codex server names no model but a reroute's, which stands for its turn;
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
        // As `codex app-server` 0.169.0 words them.
        let hear = |v: Value| on_message(&mut lock(), "reroute-1", &v);
        hear(json!({ "method": "item/started", "params": { "item": { "id": "msg_0", "type": "agentMessage", "text": "" } } }));
        turn_over("reroute-1", Some(0));
        assert_eq!(model().as_deref(), Some("gpt-a"), "the rollout, with no reroute");
        hear(json!({ "method": "item/started", "params": { "item": { "id": "msg_1", "type": "agentMessage", "text": "" } } }));
        hear(
            json!({ "method": "model/rerouted", "params": { "fromModel": "gpt-a", "toModel": "gpt-b", "reason": "highRiskCyberActivity", "threadId": "thread-reroute", "turnId": "t" } }),
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
    }

    /// A Codex helper's turn failed with `message`, as `codex app-server`
    /// 0.169.0 says it (`error`, `willRetry: false`, no `codexErrorInfo` of a
    /// limit): its error and its log after.
    fn codex_failed_on(message: &str) -> (Option<String>, Option<String>) {
        let id = "codex-limit-word";
        let mut r = lock();
        r.entries.insert(id.into(), entry(Provider::Codex));
        on_message(&mut r, id, &json!({ "method": "error", "params": { "error": { "message": message }, "willRetry": false } }));
        let e = r.entries.remove(id).unwrap();
        (e.error, e.log.back().cloned())
    }

    /// A Codex helper stopped by a usage limit: its server may say it in
    /// words alone (the message of 0.169.0's protocol/src/error.rs), kept as
    /// its word, so its record's outcome is `usageLimit`; the message itself
    /// goes to its log.
    #[test]
    fn a_codex_helpers_usage_limit_is_its_word() {
        let limit = "You\u{2019}ve hit your usage limit. Upgrade to Pro (https://chatgpt.com/explore/pro), visit https://chatgpt.com/settings/usage to purchase more credits or try again at 3:04 PM.";
        let (error, log) = codex_failed_on(limit);
        assert_eq!(error.as_deref(), Some("usage_limit"));
        assert!(log.unwrap_or_default().contains("try again at 3:04 PM"));
        assert_eq!(crate::activity::Outcome::of_error(error.as_deref()), crate::activity::Outcome::UsageLimit);
        // Straight apostrophe, and one for a model's own limit.
        let (error, _) = codex_failed_on("You've hit your usage limit for GPT-6. Switch to another model now,");
        assert_eq!(error.as_deref(), Some("usage_limit"));
        let (error, _) = codex_failed_on("stream disconnected before completion");
        assert_eq!(crate::activity::Outcome::of_error(error.as_deref()), crate::activity::Outcome::Failed);
    }

    /// Every limit 0.169.0 words (protocol/src/error.rs) is a usage limit to
    /// the record, as a chat's rollout says it; what is no limit is not.
    #[test]
    fn every_codex_limit_message_is_a_usage_limit() {
        use crate::activity::Outcome;
        let words = [
            ("You\u{2019}ve hit your usage limit.", "usage_limit"),
            (
                "You\u{2019}ve hit your usage limit. To get more access now, send a request to your admin or try again at 3:04 PM.",
                "usage_limit",
            ),
            ("You hit your spend cap set in your workspace. Increase your spend cap to continue.", "usage_limit"),
            (
                "You hit your spend cap set by the owner of your workspace. Ask an owner to increase your spend cap to continue.",
                "usage_limit",
            ),
            ("Your workspace is out of credits. Add credits to continue.", "credits_depleted"),
            ("Your workspace is out of credits. Ask your workspace owner to refill in order to continue.", "credits_depleted"),
            ("Quota exceeded. Check your plan and billing details.", "usage_limit"),
            ("To use Codex with your ChatGPT plan, upgrade to Plus: https://chatgpt.com/explore/plus.", "usage_limit"),
            ("rate limit exceeded: try again in 20s", "rate_limit"),
        ];
        for (message, word) in words {
            let (error, _) = codex_failed_on(message);
            assert_eq!(error.as_deref(), Some(word), "{message}");
            assert_eq!(Outcome::of_error(error.as_deref()), Outcome::UsageLimit, "{message}");
        }
        for other in ["budget reached", "unexpected status 500 Internal Server Error", "We\u{2019}re currently experiencing high demand"] {
            let (error, _) = codex_failed_on(other);
            assert_eq!(Outcome::of_error(error.as_deref()), Outcome::Failed, "{other}");
        }
    }

    /// A Claude Code helper stopped by its API: the stream's stand-in message
    /// says which error (2.1.325 `assistant` events carry `error`), and the
    /// failed `result` keeps that word, as a chat's `StopFailure` does, so a
    /// usage limit is `usageLimit`.
    #[test]
    fn a_claude_helpers_api_error_is_its_word() {
        use crate::activity::Outcome;
        let mut e = entry(Provider::Claude);
        e.run = Some(Run::claimed(None, 0));
        let stand_in = json!({ "type": "assistant", "error": "rate_limit", "message": { "model": "<synthetic>", "content": [{ "type": "text", "text": "You've hit your session limit" }] } });
        on_claude_event(&mut e, &stand_in);
        on_claude_event(
            &mut e,
            &json!({ "type": "result", "subtype": "success", "is_error": true, "result": "You've hit your session limit" }),
        );
        assert_eq!(e.error.as_deref(), Some("rate_limit"));
        assert_eq!(Outcome::of_error(e.error.as_deref()), Outcome::UsageLimit);
        // Another API error is its own word, and a failure.
        let mut e = entry(Provider::Claude);
        e.run = Some(Run::claimed(None, 0));
        on_claude_event(&mut e, &json!({ "type": "assistant", "error": "authentication_failed", "message": { "model": "<synthetic>" } }));
        on_claude_event(&mut e, &json!({ "type": "result", "subtype": "success", "is_error": true, "result": "Invalid API key" }));
        assert_eq!((e.error.as_deref(), Outcome::of_error(e.error.as_deref())), (Some("authentication_failed"), Outcome::Failed));
    }

    #[test]
    fn its_first_result_ends_stdin_and_text_then_waits_for_the_next_process() {
        let mut e = entry(Provider::Claude);
        let lines = feeding(&mut e);
        on_claude_event(&mut e, &json!({ "type": "result", "subtype": "success", "duration_ms": 10 }));
        assert!(matches!(lines.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected)), "the end of its input");
        assert!(e.run.is_some(), "the turn is over when its process is");
        assert_eq!(place(&mut e, "too late for it".into(), true, true), Placed::Queued);
    }

    /// As Claude Code 2.1.325 was seen: a lead's turn starts a background
    /// subagent and answers; its CLI stays up for the subagent, takes a line
    /// written then at once and answers it with a `result` of its own; once
    /// the last task is over it goes, its stdin closed.
    #[test]
    fn a_lead_takes_notices_on_stdin_while_its_own_background_tasks_run() {
        let mut e = entry(Provider::Claude);
        e.spec.tools = true;
        let lines = feeding(&mut e);
        let tasks =
            |n: usize| json!({ "type": "system", "subtype": "background_tasks_changed", "tasks": vec![json!({ "task_id": "t" }); n] });
        let result = json!({ "type": "result", "subtype": "success", "duration_ms": 10 });
        let ended = |uuid: &str| json!({ "type": "command_lifecycle", "command_uuid": uuid, "state": "completed" });
        on_claude_event(&mut e, &tasks(1));
        on_claude_event(&mut e, &result);
        on_claude_event(&mut e, &ended("opening-line"));
        assert!(lines.try_recv().is_err() && e.run.as_ref().unwrap().feed.is_some(), "its stdin stays open");
        assert!(listening(&e));

        // A notice joins, and nothing closes stdin under the turn it starts.
        let joins = listening(&e);
        let placed = place(&mut e, "[Luna notice] your helper answered".into(), joins, true);
        assert_eq!(placed, Placed::Joined);
        assert!(lines.try_recv().unwrap().contains("your helper answered"));
        assert!(!listening(&e) && !e.run.as_ref().unwrap().after_result, "at work on it");
        let uuid = e.fed[0].id.clone();
        assert!(on_claude_event(&mut e, &json!({ "type": "user", "isReplay": true, "uuid": uuid })));
        on_claude_event(
            &mut e,
            &json!({ "type": "assistant", "message": { "model": "claude-haiku", "content": [] }, "parent_tool_use_id": null }),
        );
        on_claude_event(&mut e, &result);
        on_claude_event(&mut e, &ended(&uuid));
        assert!(listening(&e), "its task runs on");

        // The last task over: stdin ends, and the CLI goes by itself.
        on_claude_event(&mut e, &tasks(0));
        assert!(matches!(lines.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected)));
        assert!(!listening(&e));
        assert_eq!(place(&mut e, "too late for it".into(), false, true), Placed::Queued);
    }

    /// Only background tasks keep the CLI up past its result: else stdin ends
    /// with it, as ever. With them it stays open for a helper too, as their
    /// asks come there (ask.rs `take_ask`), but only a lead has helpers
    /// to owe it notices.
    #[test]
    fn stdin_ends_with_the_result_of_a_helper_or_of_a_lead_with_nothing_in_the_background() {
        let tasks = json!({ "type": "system", "subtype": "background_tasks_changed", "tasks": [{ "task_id": "t" }] });
        let result = json!({ "type": "result", "subtype": "success", "duration_ms": 10 });
        for lead in [false, true] {
            let mut e = entry(Provider::Claude);
            e.spec.tools = lead;
            let lines = feeding(&mut e);
            on_claude_event(&mut e, &result);
            assert!(matches!(lines.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected)), "lead {lead}, nothing running");
            assert!(!listening(&e));

            let mut e = entry(Provider::Claude);
            e.spec.tools = lead;
            let lines = feeding(&mut e);
            on_claude_event(&mut e, &tasks);
            on_claude_event(&mut e, &result);
            assert!(lines.try_recv().is_err() && e.run.as_ref().unwrap().feed.is_some(), "lead {lead}, a task running");
            assert_eq!(listening(&e), lead, "notices for a lead alone");
            // The task over: stdin ends, and the CLI goes by itself.
            on_claude_event(&mut e, &json!({ "type": "system", "subtype": "background_tasks_changed", "tasks": [] }));
            assert!(matches!(lines.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected)), "lead {lead}");
        }
        // Nor past LISTEN_MS, at its next result.
        let mut e = entry(Provider::Claude);
        e.spec.tools = true;
        let lines = feeding(&mut e);
        on_claude_event(&mut e, &tasks);
        e.run.as_mut().unwrap().listening_since = Some(crate::throttle::now_ms().saturating_sub(LISTEN_MS));
        on_claude_event(&mut e, &result);
        assert!(matches!(lines.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected)));
    }

    /// The notifier's look closes it for a lead owed nothing now, between turns
    /// of its CLI, or one open LISTEN_MS; not one at work on a line.
    #[test]
    fn a_lead_owed_nothing_or_listening_too_long_has_its_stdin_closed() {
        let ids = ["ls-owed", "ls-free", "ls-working", "ls-long"];
        let mut lines = Vec::new();
        for id in ids {
            test_helper(id, "C:\\acc", None, true);
            lines.push(listening_for_test(id));
        }
        {
            let mut r = lock();
            r.entries.get_mut("ls-working").unwrap().run.as_mut().unwrap().after_result = false;
            r.entries.get_mut("ls-long").unwrap().run.as_mut().unwrap().listening_since =
                Some(crate::throttle::now_ms().saturating_sub(LISTEN_MS));
        }
        // Over these alone: tests side by side have leads of their own.
        {
            let now = crate::throttle::now_ms();
            let mut r = lock();
            for id in ids {
                listen_while_owed(r.entries.get_mut(id).unwrap(), ["ls-owed", "ls-long"].contains(&id), now);
            }
        }
        let closed: Vec<bool> = lines.iter().map(|l| matches!(l.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected))).collect();
        assert_eq!(closed, [false, true, false, true]);
        assert!(takes_notices("ls-owed") && !takes_notices("ls-free"));
        let mut r = lock();
        for id in ids {
            r.entries.remove(id);
        }
        drop(r);

        // A helper without Luna's tools is owed nothing ever: only LISTEN_MS
        // closes its stdin. Nor is one closed under an ask its CLI waits on
        // there, but at LISTEN_MS, which lets the ask go with it.
        let now = crate::throttle::now_ms();
        let open = |e: &mut Entry, lead: bool, asking: bool, since: u64| {
            e.spec.tools = lead;
            let lines = feeding(e);
            let run = e.run.as_mut().unwrap();
            (run.background_tasks, run.after_result, run.listening_since) = (1, true, Some(since));
            if asking {
                e.held.push_back(Held {
                    serial: 1,
                    ask: Ask::from_claude_tool("Bash", &json!({})),
                    input: json!({}),
                    tool_use_id: None,
                    answer: None,
                    reply: None,
                });
            }
            lines
        };
        for (lead, asking, since, closes) in
            [(false, false, now, false), (true, true, now, false), (false, true, now.saturating_sub(LISTEN_MS), true)]
        {
            let mut e = entry(Provider::Claude);
            let lines = open(&mut e, lead, asking, since);
            listen_while_owed(&mut e, false, now);
            let closed = matches!(lines.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected));
            assert_eq!(closed, closes, "lead {lead}, asking {asking}");
            assert_eq!(e.held.is_empty(), asking == closes, "lead {lead}, asking {asking}: what it held goes with its stdin");
        }
    }

    /// Review of issue #73: a notice written as the lead's CLI takes up a
    /// task's end in a turn of its own (its `task_notification` still on the
    /// way to Luna) is not taken in by that turn. The `result` of that turn
    /// must not read as one on the notice: no second line joins, and stdin,
    /// with nothing owed now and the last task over, stays open until the
    /// line is taken and answered.
    #[test]
    fn stdin_stays_open_while_a_line_written_to_a_listening_lead_is_untaken() {
        let id = "lu-lead";
        test_helper(id, "C:\\acc", None, true);
        let lines = listening_for_test(id);
        let event = |v: Value| claude_event_for_test(id, &v);
        let result = || json!({ "type": "result", "subtype": "success", "duration_ms": 10 });
        let tasks =
            |n: usize| json!({ "type": "system", "subtype": "background_tasks_changed", "tasks": vec![json!({ "task_id": "t" }); n] });
        let look = || listen_while_owed(lock().entries.get_mut(id).unwrap(), false, crate::throttle::now_ms());
        let open =
            |lines: &std::sync::mpsc::Receiver<String>| !matches!(lines.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected));

        assert_eq!(send_marked(id, "[Luna notice] your helper answered", None, Vec::new()), Ok((Placed::Joined, false)));
        let uuid = lock().entries[id].fed[0].id.clone();
        assert!(lines.try_recv().is_ok());
        // The CLI's own turn on the task's end, and its `result`, before the line.
        event(json!({ "type": "system", "subtype": "task_notification" }));
        event(json!({ "type": "assistant", "message": { "model": "claude-haiku", "content": [] }, "parent_tool_use_id": null }));
        event(result());
        assert!(!takes_notices(id), "the line is still to be taken");
        look();
        event(tasks(0));
        assert!(open(&lines), "nothing closes it under the line");
        assert_eq!(send(id, "from its parent", None), Ok(Placed::Joined), "a send still joins, behind the line");
        let uuid2 = lock().entries[id].fed[1].id.clone();
        let _ = lines.try_recv();

        // Taken and answered: then it goes.
        for u in [&uuid, &uuid2] {
            event(json!({ "type": "command_lifecycle", "command_uuid": u, "state": "started" }));
        }
        event(json!({ "type": "assistant", "message": { "model": "claude-haiku", "content": [] }, "parent_tool_use_id": null }));
        event(result());
        assert!(!open(&lines), "at its result, the tasks over");
        lock().entries.remove(id);
    }

    /// Second review of issue #73, in this order: a notice joins a listening
    /// lead, its last task ends while the line is untaken, and then its CLI
    /// writes the `result` of a turn of its own. Neither closes stdin under
    /// the line; its own turn's `result` does, and LISTEN_MS does regardless.
    #[test]
    fn a_result_of_the_clis_own_turn_leaves_stdin_open_under_an_untaken_line() {
        let tasks =
            |n: usize| json!({ "type": "system", "subtype": "background_tasks_changed", "tasks": vec![json!({ "task_id": "t" }); n] });
        let result = json!({ "type": "result", "subtype": "success", "duration_ms": 10 });
        let assistant = json!({ "type": "assistant", "message": { "model": "claude-haiku", "content": [] }, "parent_tool_use_id": null });
        // What was written aside: whether the far end still takes more.
        let open = |lines: &std::sync::mpsc::Receiver<String>| loop {
            if let Err(e) = lines.try_recv() {
                break e != std::sync::mpsc::TryRecvError::Disconnected;
            }
        };
        for too_long in [false, true] {
            let mut e = entry(Provider::Claude);
            e.spec.tools = true;
            let lines = feeding(&mut e);
            on_claude_event(&mut e, &tasks(1));
            on_claude_event(&mut e, &result);
            assert!(listening(&e));
            assert_eq!(place(&mut e, "[Luna notice] your helper answered".into(), true, true), Placed::Joined);
            let uuid = e.fed[0].id.clone();
            on_claude_event(&mut e, &tasks(0));
            assert!(open(&lines), "its last task over, the line untaken");
            if too_long {
                e.run.as_mut().unwrap().listening_since = Some(crate::throttle::now_ms().saturating_sub(LISTEN_MS));
            }
            // The CLI's own turn on the task's end, before the line.
            on_claude_event(&mut e, &json!({ "type": "system", "subtype": "task_notification" }));
            on_claude_event(&mut e, &assistant);
            on_claude_event(&mut e, &result);
            if too_long {
                assert!(!open(&lines), "LISTEN_MS closes it all the same");
                continue;
            }
            assert!(open(&lines), "the CLI's own result does not close it under the line");
            assert!(!listening(&e), "nor does another notice join behind the line as if at rest");

            // The line taken and answered: its result closes it.
            assert!(on_claude_event(&mut e, &json!({ "type": "command_lifecycle", "command_uuid": uuid, "state": "started" })));
            on_claude_event(&mut e, &assistant);
            on_claude_event(&mut e, &result);
            assert!(!open(&lines), "at the result of the line's own turn, the tasks over");
        }
    }

    /// What was queued for a lead goes onto its stdin at the `result` that
    /// finds it taking notices, in order, up to text its CLI would read as a
    /// command, which waits with what came after it for the next process and
    /// its sender's reach asked again (queue.rs `settle_batch`).
    #[test]
    fn a_listening_leads_queue_goes_onto_its_stdin_up_to_a_command() {
        let mut e = entry(Provider::Claude);
        e.spec.tools = true;
        let lines = feeding(&mut e);
        e.run.as_mut().unwrap().background_tasks = 1;
        let result = json!({ "type": "result", "subtype": "success", "duration_ms": 10 });
        e.queue = vec!["a notice".into(), "plain words".into(), "/compact".into(), "after it".into()];
        assert!(on_claude_event(&mut e, &result), "saved, its queue moved");
        let sent: Vec<String> = lines.try_iter().collect();
        assert_eq!(sent.len(), 2);
        assert!(sent[0].contains("a notice") && sent[1].contains("plain words"));
        assert_eq!(e.fed, ["a notice", "plain words"]);
        assert_eq!(e.queue, ["/compact", "after it"]);
    }

    /// Text moved onto a process's stdin is found by a send whose debt goes
    /// in late, the process gone by then, with the count that process ended
    /// past; not while more of its sender's text is with a later process.
    /// The last DRAINED_KEPT are kept past a process's end, and none the
    /// process never took in.
    #[test]
    fn what_went_onto_stdin_is_found_past_its_process_unless_its_sender_has_more_coming() {
        let id = "drained-kept";
        let moved = |n: usize, turns_ended: u32| Drained { id: format!("q{n}"), from: Some("p".into()), turns_ended };
        let mut e = entry(Provider::Claude);
        e.turns_ended = 3;
        e.drained = vec![moved(0, 2)];
        lock().entries.insert(id.into(), e);
        assert_eq!(moved_onto_stdin_at(id, "q0"), Some((Some("p".into()), 2)), "its process gone");
        assert_eq!(moved_onto_stdin_at(id, "q9"), None, "never moved");
        let mut later: Queued = "more of it".into();
        later.from = Some("p".into());
        lock().entries.get_mut(id).unwrap().in_flight = vec![later];
        assert_eq!(moved_onto_stdin_at(id, "q0"), None, "a later process answers the sender");
        {
            let mut r = lock();
            let e = r.entries.get_mut(id).unwrap();
            // Its process is to go before taking in what it was started on,
            // or the line moved onto its stdin: both are left undelivered,
            // and no turn starts on them here.
            e.in_flight = vec!["the batch".into()];
            let mut untaken: Queued = "never taken in".into();
            (untaken.id, untaken.from) = ("u".into(), Some("p".into()));
            e.fed = vec![untaken];
            e.drained = (0..=DRAINED_KEPT).map(|n| moved(n, 3)).collect();
            e.drained.push(Drained { id: "u".into(), from: Some("p".into()), turns_ended: 3 });
        }
        turn_over(id, Some(0));
        assert_eq!(moved_onto_stdin_at(id, "u"), None, "never taken in: no move of its sender's");
        assert_eq!(moved_onto_stdin_at(id, "q0"), None, "the oldest let go");
        assert_eq!(moved_onto_stdin_at(id, &format!("q{DRAINED_KEPT}")), Some((Some("p".into()), 3)));
        lock().entries.remove(id);
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
        assert_eq!(place(&mut e, "joined".into(), true, true), Placed::Joined);
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

        e.error = Some("it was stopped, by luna_session_stop or from the window".into());
        on_claude_event(&mut e, &done);
        assert!(e.error.is_some(), "not a result's to clear");
    }

    /// A task event as Claude Code 2.1.325 writes it on stream-json, its
    /// fields as its emitters set them (`task_started`, `task_notification`
    /// by way of its one terminal emit per task, `background_tasks_changed`).
    fn task_event(subtype: &str, id: &str, extra: Value) -> Value {
        let mut v = json!({ "type": "system", "subtype": subtype, "task_id": id, "run_id": format!("run-{id}"),
            "tool_use_id": format!("toolu_{id}"), "session_id": "s-1", "uuid": format!("u-{subtype}-{id}") });
        v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        v
    }

    fn agent_started(id: &str, description: &str, backgrounded: bool) -> Value {
        task_event(
            "task_started",
            id,
            json!({ "description": description, "subagent_type": "general-purpose", "is_backgrounded": backgrounded,
                "spawn_depth": 1, "task_type": "local_agent", "prompt": "Implement it and commit." }),
        )
    }

    fn ended(id: &str, status: &str) -> Value {
        task_event("task_notification", id, json!({ "status": status, "output_file": "", "summary": "", "usage": null }))
    }

    fn running(tasks: &[(&str, &str, &str)]) -> Value {
        let tasks: Vec<Value> = tasks
            .iter()
            .map(|(id, kind, description)| {
                json!({ "task_id": id, "run_id": format!("run-{id}"), "task_type": kind,
                "subagent_type": "general-purpose", "description": description, "parent_task_id": null })
            })
            .collect();
        json!({ "type": "system", "subtype": "background_tasks_changed", "tasks": tasks })
    }

    fn main_says() -> Value {
        json!({ "type": "assistant", "message": { "model": "claude-haiku", "content": [] }, "parent_tool_use_id": null })
    }

    fn result() -> Value {
        json!({ "type": "result", "subtype": "success", "duration_ms": 10 })
    }

    fn unfinished(e: &Entry) -> Vec<String> {
        unfinished_subagents(e.run.as_ref().unwrap())
    }

    /// A lead whose turn started background implementers: the one whose end
    /// its CLI said is over, the others went with the process, whether or not
    /// a later listing still has them. Its shells, its foreground subagents and
    /// the subagents its subagents spawned are none of them; a start that does
    /// not say where it runs is one.
    #[test]
    fn the_background_subagents_a_process_never_said_were_done_are_unfinished() {
        let mut e = entry(Provider::Claude);
        e.run = Some(Run::claimed(None, 0));
        on_claude_event(&mut e, &agent_started("a1", "Implement slice A", true));
        on_claude_event(&mut e, &agent_started("b2", "Implement slice B", true));
        on_claude_event(&mut e, &agent_started("f9", "Look something up", false));
        on_claude_event(&mut e, &task_event("task_started", "sh", json!({ "description": "gate", "task_type": "local_bash" })));
        let nested = json!({ "description": "Its helper", "task_type": "local_agent", "is_backgrounded": true, "spawn_depth": 2 });
        on_claude_event(&mut e, &task_event("task_started", "n1", nested));
        let under = json!({ "description": "Under a1", "task_type": "local_agent", "parent_task_id": "a1" });
        on_claude_event(&mut e, &task_event("task_started", "n2", under));
        on_claude_event(&mut e, &task_event("task_started", "u3", json!({ "description": "Unsaid", "task_type": "local_agent" })));
        on_claude_event(&mut e, &running(&[("a1", "local_agent", "Implement slice A"), ("b2", "local_agent", "Implement slice B")]));
        on_claude_event(&mut e, &main_says());
        on_claude_event(&mut e, &result());
        on_claude_event(&mut e, &ended("a1", "completed"));
        on_claude_event(&mut e, &running(&[("b2", "local_agent", "Implement slice B"), ("sh", "local_bash", "gate")]));
        assert_eq!(unfinished(&e), ["Implement slice B", "Unsaid"]);

        // A foreground subagent sent to the background since is listed there.
        on_claude_event(&mut e, &running(&[("b2", "local_agent", "Implement slice B"), ("f9", "local_agent", "Look something up")]));
        assert_eq!(unfinished(&e), ["Implement slice B", "Unsaid", "Look something up"]);
        // Left out of a listing, a listing with no `tasks`, an end of no
        // status or one it does not know: none of it ends a subagent.
        on_claude_event(&mut e, &running(&[]));
        on_claude_event(&mut e, &json!({ "type": "system", "subtype": "background_tasks_changed" }));
        on_claude_event(&mut e, &task_event("task_notification", "u3", json!({})));
        on_claude_event(&mut e, &ended("f9", "paused"));
        assert_eq!(unfinished(&e), ["Implement slice B", "Unsaid", "Look something up"]);
        for id in ["b2", "u3", "f9"] {
            on_claude_event(&mut e, &ended(id, "failed"));
        }
        assert!(unfinished(&e).is_empty(), "a failure is an end it said");
    }

    /// Stopped by the model mid-turn (its `TaskStop`), a subagent is over;
    /// stopped by the CLI after the turn's `result`, its wait for them run out,
    /// it is cut for good: in whatever order the stops and the listing each
    /// kill writes come.
    #[test]
    fn a_stop_after_the_turns_result_is_the_clis_in_any_order() {
        let kills = [("x", "Refactor"), ("y", "Tests"), ("z", "Docs")];
        let left_after =
            |k: usize| -> Value { running(&kills[k + 1..].iter().map(|(id, d)| (*id, "local_agent", *d)).collect::<Vec<_>>()) };
        for order in ["stop, then listing", "listing, then stop", "listings, then stops"] {
            let mut e = entry(Provider::Claude);
            e.run = Some(Run::claimed(None, 0));
            on_claude_event(&mut e, &agent_started("m", "Stopped by the model", true));
            on_claude_event(&mut e, &agent_started("c", "Done in time", true));
            for (id, d) in kills {
                on_claude_event(&mut e, &agent_started(id, d, true));
            }
            on_claude_event(&mut e, &main_says());
            on_claude_event(&mut e, &ended("m", "stopped"));
            on_claude_event(&mut e, &ended("c", "completed"));
            on_claude_event(&mut e, &result());
            match order {
                "stop, then listing" => {
                    for (k, (id, _)) in kills.iter().enumerate() {
                        on_claude_event(&mut e, &ended(id, "stopped"));
                        on_claude_event(&mut e, &left_after(k));
                    }
                }
                "listings, then stops" => {
                    for k in 0..kills.len() {
                        on_claude_event(&mut e, &left_after(k));
                    }
                    for (id, _) in kills {
                        on_claude_event(&mut e, &ended(id, "stopped"));
                    }
                }
                _ => {
                    for (k, (id, _)) in kills.iter().enumerate() {
                        on_claude_event(&mut e, &left_after(k));
                        on_claude_event(&mut e, &ended(id, "stopped"));
                    }
                }
            }
            assert_eq!(unfinished(&e), ["Refactor", "Tests", "Docs"], "{order}");
        }
    }

    /// A subagent resumed in the same process keeps its `task_id` under a new
    /// `run_id`. What comes late of its earlier run, a listing or an end,
    /// neither stands for the new run nor ends it; an end that names no run
    /// ends the one run still open, and none while two are.
    #[test]
    fn an_earlier_runs_events_never_stand_for_a_resumed_one() {
        let mut e = entry(Provider::Claude);
        e.run = Some(Run::claimed(None, 0));
        let end = |run: Option<&str>, status: &str| {
            let mut v = ended("a1", status);
            match run {
                Some(r) => v["run_id"] = r.into(),
                None => drop(v.as_object_mut().unwrap().remove("run_id")),
            }
            v
        };
        let start = |run: &str| {
            let mut v = agent_started("a1", "Implement slice A", true);
            v["run_id"] = run.into();
            v
        };
        on_claude_event(&mut e, &start("r1"));
        on_claude_event(&mut e, &end(Some("r1"), "completed"));
        assert!(unfinished(&e).is_empty());
        on_claude_event(&mut e, &start("r2"));
        assert_eq!(unfinished(&e), ["Implement slice A"], "resumed");
        let mut late = running(&[("a1", "local_agent", "Implement slice A")]);
        late["tasks"][0]["run_id"] = "r1".into();
        on_claude_event(&mut e, &late);
        on_claude_event(&mut e, &end(Some("r1"), "completed"));
        assert_eq!(unfinished(&e), ["Implement slice A"], "r1's listing and end, late");
        on_claude_event(&mut e, &end(Some("r2"), "completed"));
        assert!(unfinished(&e).is_empty(), "r2's own end");

        // An end with no run: the one run open, if only one is.
        on_claude_event(&mut e, &start("r3"));
        on_claude_event(&mut e, &end(None, "completed"));
        assert!(unfinished(&e).is_empty(), "r3, the only one open");
        on_claude_event(&mut e, &start("r4"));
        on_claude_event(&mut e, &start("r5"));
        on_claude_event(&mut e, &end(None, "completed"));
        assert_eq!(unfinished(&e), ["Implement slice A"], "two open: neither is ended, and it is named once");
    }

    /// The process gone with subagents unfinished: the turn's loss names
    /// them, says how the process went, and keeps the CLI's own word on
    /// stderr its cause. That word marks nothing by itself: a subagent whose
    /// end was still in stdout's buffer as it came is done all the same.
    #[test]
    fn a_process_gone_with_its_subagents_records_them_for_its_callers() {
        let id = "lost-subagents";
        lock().entries.insert(id.into(), entry(Provider::Claude));
        lock().entries.get_mut(id).unwrap().run = Some(Run::claimed(None, 0));
        let event = |v: Value| on_claude_event(lock().entries.get_mut(id).unwrap(), &v);
        let losses = || lock().entries[id].observed.losses.iter().map(|l| (l.turn, l.cause, l.by, l.names.clone())).collect::<Vec<_>>();
        let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        event(agent_started("a1", "Implement slice A", true));
        event(agent_started("b2", "Implement slice B", true));
        event(result());
        event(ended("a1", "completed"));
        turn_over(id, None);
        assert_eq!(losses(), [(1, StopCause::Ceiling, Some(LostBy::Exited), names(&["Implement slice B"]))]);
        assert!(!background_stopped(id), "nobody was owed anything of it: nobody to tell");

        // At the ceiling, stderr read first, A's end still on its way.
        lock().entries.get_mut(id).unwrap().run = Some(Run::claimed(None, 0));
        event(agent_started("a3", "Implement slice A", true));
        event(agent_started("c3", "Implement slice C", true));
        event(result());
        stderr_for_test(id, "Background tasks still running 3h after the last turn (2 agents: \"A\", \"C\"); stopping them. Set CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS=0 to wait indefinitely.");
        event(ended("a3", "completed"));
        event(running(&[]));
        turn_over(id, Some(0));
        assert_eq!(losses(), [(2, StopCause::Ceiling, None, names(&["Implement slice C"]))], "the earlier, told to nobody, gone");

        // Stopped by Luna.
        lock().entries.get_mut(id).unwrap().run = Some(Run { luna_stopped: true, ..Run::claimed(None, 0) });
        event(agent_started("e5", "Implement slice E", true));
        turn_over(id, None);
        assert_eq!(losses(), [(3, StopCause::Ceiling, Some(LostBy::Stopped), names(&["Implement slice E"]))]);

        // All done: nothing recorded, and the loss before stays as it was.
        lock().entries.get_mut(id).unwrap().run = Some(Run::claimed(None, 0));
        event(agent_started("d4", "Implement slice D", true));
        event(ended("d4", "completed"));
        turn_over(id, Some(0));
        assert_eq!(losses().len(), 1);
        lock().entries.remove(id);
    }

    /// Two turns each lose subagents before a caller hears of the first.
    /// Each loss keeps its own cause, and a caller told of the first hears of
    /// the second alone.
    #[test]
    fn a_caller_hears_of_each_loss_once_by_its_own_cause() {
        let id = "two-losses";
        lock().entries.insert(id.into(), entry(Provider::Claude));
        let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let record = |turn, cause, by, n: &[&str], owed: &[&str]| {
            record_loss(&mut lock().entries.get_mut(id).unwrap().observed, turn, cause, by, names(n), names(owed));
        };
        let untold = |c: &str| untold_losses(id, c).into_iter().map(|l| (l.turn, l.cause, l.names)).collect::<Vec<_>>();
        record(1, StopCause::Ceiling, Some(LostBy::Exited), &["A"], &["c1", "c2"]);
        assert_eq!(losses_told(id, "c1", &[1], &names(&["c1", "c2"])), [1]);
        record(2, StopCause::TurnEnd, None, &["B"], &["c1"]);
        assert_eq!(untold("c1"), [(2, StopCause::TurnEnd, names(&["B"]))], "the new one alone");
        assert_eq!(
            untold("c2"),
            [(1, StopCause::Ceiling, names(&["A"]))],
            "the earlier, still to be said to the one not told, by its own cause"
        );
        losses_told(id, "c1", &[2], &names(&["c1", "c2"]));
        losses_told(id, "c2", &[1], &names(&["c1", "c2"]));
        assert!(!background_stopped(id), "everyone has had each");
        record(3, StopCause::Ceiling, Some(LostBy::Stopped), &["C"], &["c3"]);
        assert_eq!(lock().entries[id].observed.losses.iter().map(|l| l.turn).collect::<Vec<_>>(), [3], "the told ones gone");
        lock().entries.remove(id);
    }

    /// The lines of a stream Claude Code 2.1.325 wrote, captured live
    /// (`testdata/`), each as the reader takes it.
    fn captured(stream: &str) -> Vec<Value> {
        stream.lines().filter(|l| !l.trim().is_empty()).map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    const INTERRUPTED_STREAM: &str = include_str!("testdata/interrupt-2.1.325.jsonl");
    const PERMISSIONS_STREAM: &str = include_str!("testdata/permissions-2.1.325.jsonl");

    /// A turn Luna asked to stop (control.rs `ask_to_stop`), as 2.1.325 ends
    /// it: its `control_response`, its tool stopped, a `result` of
    /// `error_during_execution`. That `result` is the stop's: the reason the
    /// stop set stands, or "interrupted" where it set none, and no "failed".
    /// The same stream with no interrupt asked of it is a turn that failed.
    #[test]
    fn a_turn_luna_asked_to_stop_ends_on_the_stop_not_a_failure() {
        for preset in [None, Some(STOPPED_BY_STOP)] {
            let mut e = entry(Provider::Claude);
            let _lines = feeding(&mut e);
            e.run.as_mut().unwrap().interrupt = Some("luna-int-1".into());
            e.error = preset.map(str::to_owned);
            for v in captured(INTERRUPTED_STREAM) {
                on_claude_event(&mut e, &v);
            }
            assert_eq!(e.error.as_deref(), Some(preset.unwrap_or(INTERRUPTED)), "{preset:?}");
            assert!(e.log.iter().any(|l| l == "its CLI took the interrupt"), "{:?}", e.log);
            assert!(e.log.iter().any(|l| l.starts_with("turn stopped as asked")), "{:?}", e.log);
            assert!(!e.log.iter().any(|l| l.starts_with("turn failed")), "{:?}", e.log);
            assert!(e.run.as_ref().unwrap().feed.is_none(), "its stdin ends with the result");
        }
        let mut e = entry(Provider::Claude);
        let _lines = feeding(&mut e);
        for v in captured(INTERRUPTED_STREAM) {
            on_claude_event(&mut e, &v);
        }
        assert_eq!(e.error.as_deref(), Some("error_during_execution"));
    }

    /// Its CLI's permission prompts (2.1.325, `--permission-prompt-tool
    /// stdio`) are asks to hold, never refused like its other requests; the
    /// answers Luna wrote, which it echoes, are nothing to act on.
    #[test]
    fn its_clis_permission_prompts_are_held_not_refused() {
        let stream = captured(PERMISSIONS_STREAM);
        let asks: Vec<(String, Value)> = stream.iter().filter_map(permission_request).collect();
        assert_eq!(asks.len(), 2);
        assert_eq!(asks[0].1["tool_name"], "Bash");
        assert_eq!(asks[0].1["input"]["command"], "touch a.txt");
        assert_eq!(asks[0].1["tool_use_id"], "toolu_01YTkidUi2vtGxPLPso6W7jg");
        let mut e = entry(Provider::Claude);
        let lines = feeding(&mut e);
        for v in &stream {
            on_claude_event(&mut e, v);
        }
        assert!(lines.try_recv().is_err(), "nothing written back for them here: the ask's thread answers");
        assert!(!e.log.iter().any(|l| l.starts_with("refused")), "{:?}", e.log);
    }

    /// Past a `result`, a helper's stdin stays open only for what may ask on
    /// it: a background subagent or workflow, not a shell or a monitor alone,
    /// which the CLI winds down by itself with stdin closed. Through the turn
    /// the CLI starts on such a task's end too, which may ask, until that
    /// turn's `result`. A lead owed nothing keeps it while a subagent runs.
    #[test]
    fn stdin_stays_open_past_a_result_only_for_what_may_ask() {
        let tasks = |types: &[&str]| {
            json!({ "type": "system", "subtype": "background_tasks_changed",
                "tasks": types.iter().enumerate().map(|(i, t)| json!({ "task_id": format!("t{i}"), "task_type": t })).collect::<Vec<_>>() })
        };
        let result = json!({ "type": "result", "subtype": "success", "duration_ms": 10 });
        let closed =
            |lines: &std::sync::mpsc::Receiver<String>| matches!(lines.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected));

        // A shell alone: closed at the result, as ever.
        let mut e = entry(Provider::Claude);
        let lines = feeding(&mut e);
        on_claude_event(&mut e, &tasks(&["local_bash"]));
        on_claude_event(&mut e, &result);
        assert!(closed(&lines), "a shell asks nothing");

        // A subagent: open past the result while a listing names it. Its
        // end: closed at the listing that names none, unless the CLI's own
        // turn on it has begun (its model has written), then at that turn's
        // result. Its notification alone keeps nothing open.
        let notified = json!({ "type": "system", "subtype": "task_notification", "task_id": "t0", "status": "completed" });
        let begun = json!({ "type": "assistant", "parent_tool_use_id": null, "message": { "model": "claude-haiku", "content": [] } });
        for kind in ["local_agent", "local_workflow"] {
            // The turn on its end begun before the listing: closed at its result.
            let mut e = entry(Provider::Claude);
            let lines = feeding(&mut e);
            on_claude_event(&mut e, &tasks(&[kind, "local_bash"]));
            on_claude_event(&mut e, &result);
            assert!(!closed(&lines), "{kind} may ask");
            on_claude_event(&mut e, &notified);
            assert!(!closed(&lines), "{kind}: listed still");
            on_claude_event(&mut e, &begun);
            on_claude_event(&mut e, &tasks(&["local_bash"]));
            assert!(!closed(&lines), "{kind}: the CLI's turn on its end has begun, and may ask");
            on_claude_event(&mut e, &result);
            assert!(closed(&lines), "{kind}: closed at that turn's result, the shell left to the CLI");

            // The listing first, the turn after (or none at all): closed at
            // the listing, the notification having kept nothing open.
            let mut e = entry(Provider::Claude);
            let lines = feeding(&mut e);
            on_claude_event(&mut e, &tasks(&[kind]));
            on_claude_event(&mut e, &result);
            on_claude_event(&mut e, &notified);
            on_claude_event(&mut e, &tasks(&[]));
            assert!(closed(&lines), "{kind}: no turn begun, no hold");
        }

        // A lead owed nothing: kept while its subagent runs, closed once only
        // a shell is left between its CLI's turns.
        let mut e = entry(Provider::Claude);
        e.spec.tools = true;
        let lines = feeding(&mut e);
        on_claude_event(&mut e, &tasks(&["local_agent", "local_bash"]));
        on_claude_event(&mut e, &result);
        listen_while_owed(&mut e, false, crate::throttle::now_ms());
        assert!(!closed(&lines), "a subagent of its may ask");
        e.run.as_mut().unwrap().asking_tasks = 0;
        listen_while_owed(&mut e, false, crate::throttle::now_ms());
        assert!(closed(&lines), "nothing owed, nothing that asks");
    }
}
