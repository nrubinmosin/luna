//! The running read of a chat's rollout, from where the last one stopped: the
//! turns begun and ended, the failure a turn ended on, a question left open, and
//! the subagents at work under it.

use super::*;

/// What the turn tracker of each chat has read of its rollout, and where it
/// stopped.
pub(super) struct TurnCount {
    path: PathBuf,
    /// Bytes consumed: the end of the last complete line.
    offset: u64,
    scan: Scan,
    /// The rollouts it was on before this one in this process (a `/new`, a
    /// `/resume` in its TUI), by path, as far as they were read: their turns
    /// count on with this one's, once each, and one gone back to goes on from
    /// where it was left.
    before: HashMap<PathBuf, (u64, Scan)>,
}

/// The rollout read so far, as the tracker keeps it.
#[derive(Default, Clone)]
pub(super) struct Scan {
    /// Turns ended since the process was spawned.
    pub(super) completed: u32,
    /// Whether the last turn event read started a turn that has not ended;
    /// None before any.
    pub(super) working: Option<bool>,
    /// The question the model last put to the user that nothing has answered
    /// yet, with the call id it came in.
    ask: Option<(String, crate::agents::Ask)>,
    /// Why the last turn stopped short, if it did: `task_complete` carries the
    /// error it ended on (Codex 0.166), `turn_aborted` why it was cut off.
    failed: Option<String>,
    /// What the model last said, as the last turn's end has it; None while a
    /// turn is on.
    pub(super) last_said: Option<String>,
    /// When that end was written.
    pub(super) last_said_at: Option<String>,
}

pub(super) fn turn_counts() -> &'static Mutex<HashMap<String, TurnCount>> {
    static C: OnceLock<Mutex<HashMap<String, TurnCount>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// The rollout read on from where the last look stopped. Counted off the
/// file rather than off sampled state, so a turn that starts and ends between
/// two samples of the activity tracker still counts — a one-word answer takes
/// two seconds, the sampler looks every five — and read whole rather than by
/// its tail, which a long turn outgrows: a megabyte of tool output after
/// `task_started` and the tail read the session as resting. None until the
/// rollout exists, i.e. before the first turn.
pub(super) fn tracked(live: &Live) -> Option<Scan> {
    tracked_at(live, rollout_for(live)?)
}

/// `tracked`, the chat on the rollout at `path` now.
pub(super) fn tracked_at(live: &Live, path: PathBuf) -> Option<Scan> {
    let mut counts = turn_counts().lock().ok()?;
    let entry = counts.entry(live.chat_id.to_string()).or_insert_with(|| TurnCount {
        path: path.clone(),
        offset: 0,
        scan: Scan::default(),
        before: HashMap::new(),
    });
    let since = live.spawned_at_ms as i64;
    if entry.path != path {
        // What the one it leaves wrote since the last look counts first: a turn
        // that ended just before the `/new`. Read no further after that: what
        // comes there later is another process's on that thread.
        if !past_end(&entry.path, entry.offset) {
            entry.offset = scan_rollout(&entry.path, entry.offset, since, &mut entry.scan);
        }
        let (offset, scan) = entry.before.remove(&path).unwrap_or_default();
        let was = std::mem::replace(&mut entry.path, path.clone());
        let left = (std::mem::replace(&mut entry.offset, offset), std::mem::replace(&mut entry.scan, scan));
        entry.before.insert(was, left);
    }
    if past_end(&path, entry.offset) {
        // Cut short or written anew under the same name: what was read of it
        // no longer lines up, so it is read again from the top. The count
        // goes on from the higher of the two. Started over lower, it would
        // sit under the count `activity` already gave out (which only grows)
        // until it caught up, and a wait for the next turn would not wake;
        // added on top, turns the new file holds that were counted already
        // would count twice and wake one early. Old turn ends written back
        // after this still count as new ones; a floor under the file's own
        // count would not, but would hold back a real new turn the same way
        // starting over lower does. The CLIs only append, so a rollout cut
        // short is rare, and one that later gets its old turns back rarer.
        let mut scan = Scan::default();
        let offset = scan_rollout(&path, 0, since, &mut scan);
        scan.completed = scan.completed.max(entry.scan.completed);
        (entry.offset, entry.scan) = (offset, scan);
    } else {
        entry.offset = scan_rollout(&path, entry.offset, since, &mut entry.scan);
    }
    let mut scan = entry.scan.clone();
    scan.completed += entry.before.values().map(|(_, s)| s.completed).sum::<u32>();
    Some(scan)
}

/// Whether `offset` lies past the end of the file: a cursor into one since
/// cut short or written anew.
pub(super) fn past_end(path: &Path, offset: u64) -> bool {
    std::fs::metadata(path).is_ok_and(|m| offset > m.len())
}

/// How many turns this process of the session has finished, per its rollout:
/// one per `task_complete` (`turn_complete` / `turn_aborted` before 0.157)
/// written since it was spawned — a resumed thread's rollout holds the turns
/// of every earlier run too.
pub fn turns_completed(live: &Live) -> Option<u32> {
    tracked(live).map(|s| s.completed)
}

/// Why this chat's last turn stopped short, if it did, as of the rollout's last
/// read: the one the turn count comes from, so it goes with the turn counted,
/// and the session may have exited since.
pub fn last_failure(chat_id: &str) -> Option<String> {
    turn_counts().lock().ok()?.get(chat_id)?.scan.failed.clone()
}

/// The question the session has put to the user and nobody has answered:
/// `request_user_input`, which holds the turn until it is answered, or
/// `request_user_input_async`, which lets the model work on and is open until
/// the user next says something.
pub fn pending_ask(live: &Live) -> Option<crate::agents::Ask> {
    tracked(live)?.ask.map(|(_, a)| a)
}

/// What a chat's subagents have shown of themselves (`children_at_work`).
#[derive(Default)]
pub(super) struct Children {
    /// Each rollout looked at, with the thread its first line says spawned it:
    /// None for one that is no subagent's. A first line never changes.
    parent_of: HashMap<PathBuf, Option<String>>,
    /// A subagent's rollout as read so far, and where the read stopped.
    read: HashMap<PathBuf, (u64, Scan)>,
}

pub(super) fn children() -> &'static Mutex<HashMap<String, Children>> {
    static C: OnceLock<Mutex<HashMap<String, Children>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// The thread a rollout's first line says spawned it, if it is a subagent's.
pub(super) fn spawned_by(meta: &Value) -> Option<String> {
    meta["payload"]["source"]["subagent"]["thread_spawn"]["parent_thread_id"].as_str().map(str::to_owned)
}

/// A live session's subagents at work, by thread id. Codex (0.166) runs them
/// in the session's own process and says nothing of them on its screen or in
/// its own turns: each writes a rollout of its own beside the session's, whose
/// first line names the thread that spawned it, and is at work while the last
/// turn there has not ended. One done with its task stays open, asleep until it
/// is written to, and is not at work; one it spawned in turn counts too. All of
/// them go with the process. None until the session's own rollout is known.
///
/// Only threads begun since the process was spawned, so not one the session
/// spawned in an earlier run and wakes now, nor the subagents of a thread it
/// has left (`/new`, `/resume`), whatever becomes of those.
pub fn children_at_work(live: &Live) -> Option<Vec<String>> {
    let own = rollout_for(live)?;
    let (_, thread) = parse_name(&own.file_name()?.to_string_lossy())?;
    let spawned = Local.timestamp_millis_opt(live.spawned_at_ms as i64).single()?.naive_local();
    // A subagent's thread is begun after the session's process, give or take
    // the clocks, in the folder of the day it was begun: every day since the
    // spawn, for a session days old whose subagent from its first day is
    // woken now. Two weeks at most.
    let not_before = spawned - chrono::Duration::seconds(5);
    let today = Local::now().date_naive();
    let days = (today - not_before.date()).num_days().clamp(0, 13) as usize + 1;
    let files: Vec<(String, PathBuf)> = rollouts_on(live.account_path, days_back(today, days))
        .into_iter()
        .filter(|(ts, _, _)| *ts >= not_before)
        .map(|(_, id, p)| (id, p))
        .collect();
    let listed: std::collections::HashSet<&PathBuf> = files.iter().map(|(_, p)| p).collect();
    let mut all = children().lock().ok()?;
    let seen = all.entry(live.chat_id.to_string()).or_default();
    seen.parent_of.retain(|p, _| listed.contains(p));
    seen.read.retain(|p, _| listed.contains(p));
    for (_, path) in &files {
        if !seen.parent_of.contains_key(path) {
            // Not kept while its first line does not read yet: a file just
            // begun may hold half of it.
            if let Some(meta) = first_line(path) {
                seen.parent_of.insert(path.clone(), spawned_by(&meta));
            }
        }
    }
    // The session's thread and every thread spawned under it, at any depth.
    let mut family = std::collections::HashSet::from([thread]);
    let mut kids: Vec<(String, PathBuf)> = Vec::new();
    loop {
        let found = files.iter().filter(|(id, path)| {
            !family.contains(id) && seen.parent_of.get(path).and_then(Option::as_ref).is_some_and(|parent| family.contains(parent))
        });
        let found: Vec<(String, PathBuf)> = found.cloned().collect();
        if found.is_empty() {
            break;
        }
        family.extend(found.iter().map(|(id, _)| id.clone()));
        kids.extend(found);
    }
    let since = live.spawned_at_ms as i64;
    let mut at_work = Vec::new();
    for (id, path) in kids {
        let (offset, scan) = seen.read.entry(path.clone()).or_default();
        if past_end(&path, *offset) {
            (*offset, *scan) = (0, Scan::default());
        }
        *offset = scan_rollout(&path, *offset, since, scan);
        if scan.working == Some(true) {
            at_work.push(id);
        }
    }
    Some(at_work)
}

/// Reads the complete lines from byte `from` on into `scan` and returns the
/// offset just past the last of them. Turn ends count only when written at or
/// after `since_ms`; a line whose timestamp does not read is taken as new.
pub(super) fn scan_rollout(path: &Path, from: u64, since_ms: i64, scan: &mut Scan) -> u64 {
    crate::pty::complete_lines(path, from, |line| {
        let event = line.contains("\"event_msg\"");
        if !event && !line.contains("request_user_input") && !line.contains("\"function_call_output\"") {
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { return };
        let payload = &v["payload"];
        let at_ms = v["timestamp"].as_str().and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()).map(|t| t.timestamp_millis());
        let new = at_ms.is_none_or(|t| t >= since_ms);
        match (v["type"].as_str(), payload["type"].as_str()) {
            (Some("event_msg"), Some("task_started" | "turn_started")) => {
                // A turn an earlier process was on went with it, and Codex (0.166)
                // writes no end for it as it resumes: it sits at its composer.
                scan.working = Some(new);
                scan.failed = None;
                scan.last_said = None;
                scan.last_said_at = None;
            }
            (Some("event_msg"), Some(end @ ("task_complete" | "turn_complete" | "turn_aborted" | "task_aborted"))) => {
                scan.working = Some(false);
                scan.last_said = payload["last_agent_message"].as_str().map(str::to_owned);
                scan.last_said_at = v["timestamp"].as_str().map(str::to_owned);
                scan.failed = payload["error"]["message"].as_str().map(str::to_owned).or_else(|| {
                    end.ends_with("_aborted").then(|| match payload["reason"].as_str() {
                        Some(why) => format!("its turn was aborted ({why})"),
                        None => "its turn was aborted".to_string(),
                    })
                });
                // A blocking question goes with the turn that asked it.
                if scan.ask.as_ref().is_some_and(|(_, a)| a.blocking) {
                    scan.ask = None;
                }
                if new {
                    scan.completed += 1;
                }
            }
            // Whatever the user says next is the answer, typed or picked:
            // `user_message` before 0.163, a completed `UserMessage` item since.
            (Some("event_msg"), Some("user_message")) => scan.ask = None,
            (Some("event_msg"), Some("item_completed")) if payload["item"]["type"] == "UserMessage" => scan.ask = None,
            (Some("response_item"), Some("function_call")) => {
                let blocking = match payload["name"].as_str() {
                    Some("request_user_input") => true,
                    Some("request_user_input_async") => false,
                    _ => return,
                };
                let args = payload["arguments"].as_str().and_then(|a| serde_json::from_str::<Value>(a).ok());
                if let Some(ask) = args.as_ref().and_then(|a| ask_of(a, blocking)) {
                    scan.ask = Some((payload["call_id"].as_str().unwrap_or("").to_string(), ask));
                }
            }
            (Some("response_item"), Some("function_call_output")) => {
                let answered = scan.ask.as_ref().is_some_and(|(id, a)| a.blocking && payload["call_id"].as_str() == Some(id));
                if answered {
                    scan.ask = None;
                }
            }
            _ => {}
        }
    })
}

/// `{"questions":[…]}` as Codex's question tools take it: each with its text
/// under `question` (`title` in the async one) and the suggested answers
/// under `options`.
fn ask_of(args: &Value, blocking: bool) -> Option<crate::agents::Ask> {
    let questions: Vec<crate::agents::AskQuestion> = args["questions"]
        .as_array()?
        .iter()
        .filter_map(|q| {
            let text = q["question"].as_str().or(q["title"].as_str())?.trim().to_string();
            let options = q["options"]
                .as_array()
                .map(|o| o.iter().filter_map(|o| o["label"].as_str().or(o.as_str()).map(str::to_owned)).collect())
                .unwrap_or_default();
            (!text.is_empty()).then_some(crate::agents::AskQuestion { text, options, multi: false, list_at: None })
        })
        .collect();
    (!questions.is_empty()).then(|| crate::agents::Ask::question(questions, blocking))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session's subagents, at any depth, are at work while the last turn
    /// their own rollout opened has not ended. One done with its task sleeps
    /// until it is written to again, and another session's are none of its.
    #[test]
    fn a_sessions_subagents_are_at_work_while_their_own_turn_is_open() {
        let (root, t0) = rollout_bench("children");
        let acct = root.to_string_lossy().into_owned();
        let chat = format!("chat-children-{t0}");
        let (own, kid, grandkid, stranger) = (format!("own{t0}"), format!("kid{t0}"), format!("grand{t0}"), format!("stranger{t0}"));
        write_rollout(&root, t0 + 1_000, &own, Some(r#""cli""#));
        let live = Live { chat_id: &chat, cwd: "E:\\p", spawned_at_ms: t0 as u128, resume: None, account_path: &acct, last_output_ms: 0 };
        let at_work = || {
            let mut kids = children_at_work(&live).unwrap();
            kids.sort();
            kids
        };
        assert!(at_work().is_empty());
        write_rollout(&root, t0 + 2_000, &kid, Some(&spawned_by_json(&own)));
        write_rollout(&root, t0 + 3_000, &grandkid, Some(&spawned_by_json(&kid)));
        write_rollout(&root, t0 + 3_000, &stranger, Some(&spawned_by_json("someone-else")));
        let (kid_file, grandkid_file) = (rollout_path(&root, &kid, t0 + 2_000), rollout_path(&root, &grandkid, t0 + 3_000));
        assert!(at_work().is_empty(), "begun, with no turn yet");
        append(kid_file.clone(), turn_event("task_started", t0 + 4_000));
        append(grandkid_file.clone(), turn_event("task_started", t0 + 5_000));
        append(rollout_path(&root, &stranger, t0 + 3_000), turn_event("task_started", t0 + 5_000));
        assert_eq!(at_work(), vec![grandkid.clone(), kid.clone()], "its own, at any depth");
        append(grandkid_file.clone(), turn_event("task_complete", t0 + 6_000));
        assert_eq!(at_work(), vec![kid.clone()], "done with its task: asleep");
        append(grandkid_file.clone(), turn_event("task_started", t0 + 7_000));
        assert_eq!(at_work(), vec![grandkid.clone(), kid.clone()], "woken by a message");
        append(kid_file, turn_event("turn_aborted", t0 + 8_000));
        append(grandkid_file, turn_event("task_complete", t0 + 8_000));
        assert!(at_work().is_empty());
        release(&chat);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A session days old: a subagent begun on its first day, in that day's
    /// folder, is still its own when woken now.
    #[test]
    fn a_subagent_from_an_earlier_day_of_the_session_is_found() {
        let (root, now) = rollout_bench("old-kid");
        let acct = root.to_string_lossy().into_owned();
        let t0 = now - 3 * 86_400_000;
        let chat = format!("chat-oldkid-{t0}");
        let (own, kid) = (format!("ownold{t0}"), format!("kidold{t0}"));
        write_rollout(&root, t0 + 1_000, &own, Some(r#""cli""#));
        write_rollout(&root, t0 + 2_000, &kid, Some(&spawned_by_json(&own)));
        append(rollout_path(&root, &kid, t0 + 2_000), turn_event("task_started", now));
        let live = Live { chat_id: &chat, cwd: "E:\\p", spawned_at_ms: t0 as u128, resume: None, account_path: &acct, last_output_ms: 0 };
        // On its own thread since its first day, as its TUI said then.
        assert!(rollout_on(&live, Some(own.clone())).is_some());
        assert_eq!(children_at_work(&live), Some(vec![kid.clone()]));
        release(&chat);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Killed mid-turn with its core and resumed: Codex 0.166 sits at its composer
    /// and writes no end for that turn, which must not read as one on now.
    #[test]
    fn a_turn_an_earlier_process_was_on_is_not_running() {
        let dir = std::env::temp_dir().join(format!("luna-rollout-resumed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("r.jsonl");
        let line =
            |at: &str, kind: &str| format!("{{\"timestamp\":\"{at}\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"{kind}\"}}}}\n");
        std::fs::write(&path, line("2026-11-01T08:00:00.000Z", "task_started")).unwrap();
        let since = chrono::DateTime::parse_from_rfc3339("2026-11-01T09:00:00.000Z").unwrap().timestamp_millis();
        let mut scan = Scan::default();
        let at = scan_rollout(&path, 0, since, &mut scan);
        assert_eq!(scan.working, Some(false));
        // The first turn this process runs reads as one.
        let more = line("2026-11-01T09:00:05.000Z", "task_started");
        std::fs::write(&path, format!("{}{more}", line("2026-11-01T08:00:00.000Z", "task_started"))).unwrap();
        scan_rollout(&path, at, since, &mut scan);
        assert_eq!(scan.working, Some(true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The 0.157 rollout: `task_started` / `task_complete` for the turn, and
    /// the conversation as `response_item` messages with roles.
    #[test]
    fn reads_the_current_rollout_format() {
        let root = std::env::temp_dir().join(format!("luna-codex-fmt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("rollout-2026-09-26T15-43-59-abc.jsonl");
        let lines = [
            r#"{"timestamp":"t","type":"session_meta","payload":{"id":"abc","cwd":"E:\\p"}}"#,
            r#"{"timestamp":"t","type":"event_msg","payload":{"type":"task_started","turn_id":"t1"}}"#,
            r#"{"timestamp":"t","type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"<skills_instructions>…"}]}}"#,
            r##"{"timestamp":"t","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"# AGENTS.md instructions for E:\\p\n\nrules"}]}}"##,
            r#"{"timestamp":"t","type":"turn_context","payload":{"turn_id":"t1","cwd":"E:\\p","model":"gpt-6-astra"}}"#,
            r#"{"timestamp":"t","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Reply with the single word READY."}]}}"#,
            r#"{"timestamp":"t","type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","text":"Reply with the single word READY."}}}"#,
            r#"{"timestamp":"t","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"READY"}]}}"#,
            r#"{"timestamp":"t","type":"event_msg","payload":{"type":"task_complete","turn_id":"t1","last_agent_message":"READY"}}"#,
            // A helper's next turn, with the note Luna puts on each.
            &format!(
                r#"{{"timestamp":"t","type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":{}}}]}}}}"#,
                serde_json::to_string(&crate::pty::helper_prompt("p-1", "And now?")).unwrap()
            ),
        ];
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();

        let t = read_tail(&path);
        assert_eq!(t.model.as_deref(), Some("gpt-6-astra"));
        assert_eq!(read_first_prompt(&path).as_deref(), Some("Reply with the single word READY."));

        let (msgs, cursor) = messages_from(&path, 0);
        let got: Vec<(&str, &str)> = msgs.iter().map(|m| (m.role.as_str(), m.text.as_str())).collect();
        assert_eq!(
            got,
            vec![("user", "Reply with the single word READY."), ("assistant", "READY"), ("user", "And now?")],
            "as asked, without Luna's note"
        );
        assert_eq!(cursor, std::fs::metadata(&path).unwrap().len());
        let (more, _) = messages_from(&path, cursor);
        assert!(more.is_empty(), "nothing new after the cursor");
        // A reply still being written is not read, nor stepped over.
        let reply = r#"{"timestamp":"t","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"DONE"}]}}"#;
        let whole = lines.join("\n") + "\n";
        std::fs::write(&path, format!("{whole}{}", &reply[..40])).unwrap();
        assert_eq!(messages_from(&path, cursor), (vec![], cursor));
        std::fs::write(&path, format!("{whole}{reply}\n")).unwrap();
        let (more, _) = messages_from(&path, cursor);
        assert_eq!(more, vec![crate::agents::Message { role: "assistant".into(), text: "DONE".into(), after_cli: false }]);
        std::fs::write(&path, &whole).unwrap();

        // Turn ends are counted off complete lines only, and incrementally.
        let mut s = Scan::default();
        let consumed = scan_rollout(&path, 0, 0, &mut s);
        assert_eq!((s.completed, s.working), (1, Some(false)));
        assert_eq!(consumed, std::fs::metadata(&path).unwrap().len());
        let mut grown = std::fs::read_to_string(&path).unwrap();
        grown.push_str(r#"{"timestamp":"t","type":"event_msg","payload":{"type":"task_complete","turn_id":"t2"}}"#);
        std::fs::write(&path, &grown).unwrap();
        assert_eq!(scan_rollout(&path, consumed, 0, &mut s), consumed, "a line without its newline is not there yet");
        assert_eq!(s.completed, 1);
        grown.push('\n');
        std::fs::write(&path, &grown).unwrap();
        scan_rollout(&path, consumed, 0, &mut s);
        assert_eq!(s.completed, 2);

        // A resumed thread: the turns of an earlier run are not this one's.
        let resumed = [
            r#"{"timestamp":"2026-09-26T15:44:00.000Z","type":"event_msg","payload":{"type":"task_complete","turn_id":"t1"}}"#,
            r#"{"timestamp":"2026-09-26T16:10:00.500Z","type":"event_msg","payload":{"type":"task_complete","turn_id":"t2"}}"#,
        ];
        std::fs::write(&path, resumed.join("\n") + "\n").unwrap();
        let spawned = chrono::DateTime::parse_from_rfc3339("2026-09-26T16:00:00Z").unwrap().timestamp_millis();
        let count = |since: i64| {
            let mut s = Scan::default();
            scan_rollout(&path, 0, since, &mut s);
            s.completed
        };
        assert_eq!(count(spawned), 1);
        assert_eq!(count(0), 2);

        // Mid-turn, however much the turn has written since it started: a
        // megabyte of tool output still reads as working.
        let filler = format!(
            r#"{{"timestamp":"t","type":"response_item","payload":{{"type":"function_call_output","call_id":"c","output":"{}"}}}}"#,
            "x".repeat(4096)
        );
        let long: Vec<String> = lines[..2].iter().map(|l| l.to_string()).chain(std::iter::repeat_n(filler, 300)).collect();
        std::fs::write(&path, long.join("\n") + "\n").unwrap();
        let mut s = Scan::default();
        scan_rollout(&path, 0, 0, &mut s);
        assert_eq!(s.working, Some(true));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// How a turn that failed reads in a 0.166 rollout (a bad key, seen on a
    /// throwaway account): a `task_complete` like any other, with the error.
    #[test]
    fn a_turn_that_failed_says_why() {
        let root = std::env::temp_dir().join(format!("luna-codex-failed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("rollout-2026-11-01T10-52-43-abc.jsonl");
        let failed_after = |lines: &[&str]| {
            std::fs::write(&path, lines.join("\n") + "\n").unwrap();
            let mut s = Scan::default();
            scan_rollout(&path, 0, 0, &mut s);
            (s.failed, s.completed)
        };
        let started = r#"{"timestamp":"t","type":"event_msg","payload":{"type":"task_started","turn_id":"t1"}}"#;
        let failed = r#"{"timestamp":"t","ordinal":14,"type":"event_msg","payload":{"type":"task_complete","turn_id":"t1","last_agent_message":null,"error":{"message":"unexpected status 401 Unauthorized"}}}"#;
        let fine = r#"{"timestamp":"t","type":"event_msg","payload":{"type":"task_complete","turn_id":"t2","last_agent_message":"READY"}}"#;
        let aborted = r#"{"timestamp":"t","type":"event_msg","payload":{"type":"turn_aborted","turn_id":"t3","reason":"interrupted"}}"#;
        assert_eq!(failed_after(&[started, failed]), (Some("unexpected status 401 Unauthorized".into()), 1), "it ends a turn all the same");
        assert_eq!(failed_after(&[started, failed, started]), (None, 1), "the next turn is another one");
        assert_eq!(failed_after(&[started, failed, started, fine]), (None, 2));
        assert_eq!(failed_after(&[started, aborted]).0.as_deref(), Some("its turn was aborted (interrupted)"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Codex 0.163 asks through `request_user_input_async` (the turn goes on)
    /// and `request_user_input` (the turn waits for the answer).
    #[test]
    fn a_question_is_open_until_it_is_answered() {
        let root = std::env::temp_dir().join(format!("luna-codex-ask-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("rollout-2026-10-22T04-51-02-abc.jsonl");
        let write = |lines: &[&str]| std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        let ask = |lines: &[&str]| {
            write(lines);
            let mut s = Scan::default();
            scan_rollout(&path, 0, 0, &mut s);
            s.ask.map(|(_, a)| a)
        };
        let started = r#"{"timestamp":"t","type":"event_msg","payload":{"type":"task_started"}}"#;
        let async_q = r#"{"timestamp":"t","type":"response_item","payload":{"type":"function_call","name":"request_user_input_async","arguments":"{\"questions\":[{\"title\":\"Can you paste CLAUDE.md here?\"}]}","call_id":"call_a"}}"#;
        let accepted = r#"{"timestamp":"t","type":"response_item","payload":{"type":"function_call_output","call_id":"call_a","output":"{\"accepted\":true}"}}"#;
        let done = r#"{"timestamp":"t","type":"event_msg","payload":{"type":"task_complete"}}"#;
        let user = r#"{"timestamp":"t","type":"event_msg","payload":{"type":"user_message","message":"here it is"}}"#;

        let a = ask(&[started, async_q, accepted, done]).expect("an async question outlives its turn");
        assert!(!a.blocking);
        assert_eq!(a.questions[0].text, "Can you paste CLAUDE.md here?");
        assert!(ask(&[started, async_q, accepted, done, user]).is_none(), "the user's next word answers it");
        let item = r#"{"timestamp":"t","type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","id":"u"}}}"#;
        assert!(ask(&[started, async_q, accepted, item]).is_none(), "as 0.163 writes it");
        let plain = r#"{"timestamp":"t","type":"response_item","payload":{"type":"function_call","name":"request_user_input_async","arguments":"{\"questions\":[{\"title\":\"Size?\",\"options\":[\"Small\",\"Large\"]}]}","call_id":"c"}}"#;
        assert_eq!(ask(&[started, plain]).unwrap().questions[0].options, ["Small", "Large"], "options as bare strings");

        let blocking = r#"{"timestamp":"t","type":"response_item","payload":{"type":"function_call","name":"request_user_input","arguments":"{\"questions\":[{\"id\":\"color\",\"header\":\"Color\",\"question\":\"Which color?\",\"options\":[{\"label\":\"Red (Recommended)\",\"description\":\"\"},{\"label\":\"Blue\",\"description\":\"\"}]}]}","call_id":"call_b"}}"#;
        let answered = r#"{"timestamp":"t","type":"response_item","payload":{"type":"function_call_output","call_id":"call_b","output":"{\"answers\":{}}"}}"#;
        let a = ask(&[started, blocking]).expect("a blocking question");
        assert!(a.blocking);
        assert_eq!(a.questions[0].options, ["Red (Recommended)", "Blue"]);
        assert!(ask(&[started, blocking, answered]).is_none(), "its output is the answer");
        assert!(ask(&[started, blocking, done]).is_none(), "it goes with the turn");

        let _ = std::fs::remove_dir_all(&root);
    }
}
