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
    }
}

/// How long a lead's stdin stays open past its turn's `result` at most
/// (`keeps_listening`). The CLI's own wait for its background work
/// (command.rs `BG_WAIT_CEILING_MS`) holds only once stdin is closed, and
/// counts from there (2.1.325): this bounds the stretch before.
pub(super) const LISTEN_MS: u64 = 10_800_000;

/// Whether a lead's stdin stays open past a `result`, for the notices its
/// helpers owe it to go in as stream-json lines (queue.rs `send_as`): while
/// its CLI says it has background tasks running (subagents, shells,
/// monitors), which keep `claude -p` up after the result in any case, and for
/// LISTEN_MS at most. Claude Code 2.1.325 takes such a line at once and
/// answers it with a turn and a `result` of its own, the background work
/// running on. With stdin open it never goes by itself, so stdin is closed
/// once the last task ends, at a `result` with none left, or when nothing is
/// owed to the lead any more (`listen_only_while_owed`). Only a lead has Luna
/// helpers to owe it. Each of those closes waits for the lines written to it
/// to be taken in (`holds_untaken`); only LISTEN_MS does not.
fn keeps_listening(e: &Entry, now: u64) -> bool {
    e.spec.tools
        && e.run.as_ref().is_some_and(|run| {
            run.background_tasks > 0 && run.feed.is_some() && run.listening_since.is_none_or(|since| now.saturating_sub(since) < LISTEN_MS)
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
    !e.held_back()
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
    let untaken = !e.fed.is_empty();
    let Some(run) = e.run.as_mut() else { return };
    let Some(since) = run.listening_since else { return };
    let why = if now.saturating_sub(since) >= LISTEN_MS {
        format!("it has been open for {} hours", LISTEN_MS / 3_600_000)
    } else if untaken {
        return;
    } else if run.after_result && !owed {
        "nothing is owed to it now".to_string()
    } else {
        return;
    };
    stop_listening(run);
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
    if !listening(e) {
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
        made_calls(e, v);
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
        (Some("system"), Some("background_tasks_changed")) => {
            let running = v["tasks"].as_array().map_or(0, Vec::len);
            let untaken = !e.fed.is_empty();
            let mut closed = false;
            if let Some(run) = e.run.as_mut() {
                run.background_tasks = running;
                // Its last task is over: the CLI takes that end up by itself
                // and, its stdin closed, goes once that turn and any it is at
                // are done. Not with a line written to it still to be taken
                // in: the `result` of the turn on it closes stdin then.
                if running == 0 && !untaken && run.listening_since.is_some() {
                    stop_listening(run);
                    closed = true;
                }
            }
            if closed {
                note(e, "its background tasks are over: its stdin is closed");
            }
        }
        // A background task's end, which the CLI takes up in a turn of its
        // own (2.1.325): at work from here, ahead of that turn's first
        // `assistant`, so that a notice does not land in it (`listening`).
        (Some("system"), Some("task_notification")) => {
            if let Some(run) = e.run.as_mut() {
                run.after_result = false;
            }
        }
        (Some("assistant"), _) if v["parent_tool_use_id"].is_null() => {
            // At work on a turn again: one the end of a background job started.
            if let Some(run) = e.run.as_mut() {
                run.after_result = false;
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
            // Moved from its queue to its stdin: saved as such, so that a core
            // gone meanwhile settles it against the conversation (persist.rs).
            return feed_queued(e);
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
    let (job, strays, codex) = match lock().entries.get(id) {
        Some(e) => (
            e.job.clone(),
            e.run.as_ref().filter(|_| !e.observed.acted).and_then(|run| Some((run.pid?, run.started_ms))),
            e.session
                .clone()
                .filter(|_| e.spec.provider == Provider::Codex)
                .zip(e.run.as_ref())
                .map(|(thread, run)| (e.spec.account_path.clone(), thread, run.started_ms)),
        ),
        None => (None, None, None),
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
    // So too the subagents `codex exec` interrupted as the turn ended (0.167.2),
    // their work unfinished, with whoever is owed an answer now to hear of it:
    // the registry's lock goes before this one.
    let cut_off = codex
        .filter(|(account, thread, since)| crate::codex::session::subagents_cut_at_turn_end(account, thread, *since) > 0)
        .map(|_| crate::agents::callers_owed(id));
    let (next, thread, generation, rerouted, acted, retried, count, told_move) = {
        let mut r = lock();
        let Some(e) = r.entries.get_mut(id) else { return };
        let told_move = e.run.take().and_then(|run| run.moved);
        e.turns_ended += 1;
        // A send whose text this process took may have its debt still to go
        // in. What it never took in is no move any more: it goes again with
        // the next process (`sort_out_untaken`, below), which answers it.
        let untaken = &e.fed;
        e.drained.retain(|d| !untaken.iter().any(|q| q.id == d.id));
        let over = e.drained.len().saturating_sub(DRAINED_KEPT);
        e.drained.drain(..over);
        e.observed.turn_ended_ms = crate::throttle::now_ms();
        if let Some(callers) = cut_off {
            note(e, "codex exec interrupted the subagents of the turn as it ended");
            let o = &mut e.observed;
            o.background_stopped = !callers.is_empty();
            o.stopped_turn = e.turns_ended;
            o.stop_cause = StopCause::TurnEnd;
            o.stop_owed = callers;
            o.stop_told.clear();
        }
        e.held.clear();
        e.left_running = !left.is_empty();
        e.left = left_of(left);
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
        // Held (being moved to another chat, opened as a chat, deleted:
        // `Entry::held_back`), what is queued waits for the hold to let go
        // (control.rs `go_on`): a turn started now would have the parent and
        // the owners of before.
        let next = (!e.queue.is_empty() && !e.held_back()).then(|| {
            claim(e);
            reserve(e)
        });
        let thread = e.session.clone().filter(|_| e.spec.provider == Provider::Codex).map(|t| (e.spec.account_path.clone(), t));
        e.observed.answered |= e.observed.acted;
        let told_move = told_move.filter(|_| e.observed.acted);
        (next, thread, e.observed.generation, std::mem::take(&mut e.observed.rerouted), e.observed.acted, retried, e.turns_ended, told_move)
    };
    // A turn shorter than the reads between them was work all the same: a
    // power-off's quiet window starts over.
    crate::activity::touch();
    // Its model answered a prompt that told it of a move to another chat: no
    // turn after is told again (agents/reparent.rs `move_told`). Before the
    // next turn starts below, which would tell it again otherwise.
    if let Some(m) = told_move {
        crate::agents::move_told(id, &m);
    }
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
        let placed = place(&mut e, "[Luna notice] your helper answered".into(), joins);
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
        assert_eq!(place(&mut e, "too late for it".into(), false), Placed::Queued);
    }

    /// Only a lead has helpers to owe it notices, and only background tasks keep
    /// the CLI up past its result: else stdin ends with it, as ever.
    #[test]
    fn stdin_ends_with_the_result_of_a_helper_or_of_a_lead_with_nothing_in_the_background() {
        let tasks = json!({ "type": "system", "subtype": "background_tasks_changed", "tasks": [{ "task_id": "t" }] });
        let result = json!({ "type": "result", "subtype": "success", "duration_ms": 10 });
        for (lead, running) in [(false, true), (true, false)] {
            let mut e = entry(Provider::Claude);
            e.spec.tools = lead;
            let lines = feeding(&mut e);
            if running {
                on_claude_event(&mut e, &tasks);
            }
            on_claude_event(&mut e, &result);
            assert!(matches!(lines.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected)), "lead {lead}, running {running}");
            assert!(!listening(&e));
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
            assert_eq!(place(&mut e, "[Luna notice] your helper answered".into(), true), Placed::Joined);
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

        e.error = Some("it was stopped, by luna_session_stop or from the window".into());
        on_claude_event(&mut e, &done);
        assert!(e.error.is_some(), "not a result's to clear");
    }
}
