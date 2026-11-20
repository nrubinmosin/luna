//! What a hook says of a session (hub.rs hands each one over), a Claude Code
//! chat's or a Codex chat's (codex/hooks.rs): the turn begun, waiting on a
//! person, over, failed or cut off, and a subagent's word apart from the
//! turn's. The two CLIs' events share their names and the fields read here.

use super::*;

/// What a hook told us.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HookEvent {
    /// `PostToolUse`: the model is at work.
    Busy,
    /// `UserPromptSubmit`: a prompt went in, and the model is at work.
    Submitted,
    /// `PermissionRequest`; Claude Code's `Notification` with
    /// `permission_prompt` or `elicitation_dialog`, `PreToolUse` of
    /// `AskUserQuestion`.
    Waiting,
    /// `Stop`: the turn is over, nothing left running beside it.
    Idle,
    /// `Stop` that lists background tasks (`background_tasks`: shells,
    /// background agents) or session crons (`session_crons`) still there: the
    /// turn is over, the session's work is not.
    IdleWith { tasks: u32, crons: u32 },
    /// `StopFailure`: the turn is over because the API refused it — a usage
    /// limit, a lost login. No `Stop` follows.
    Failed(&'static str),
    /// Codex's `Interrupt`: the person cut the turn off; no `Stop` follows.
    Interrupted,
    /// `SessionEnd`.
    Gone,
    /// A subagent of the session (the hook carries its `agent_id`) has
    /// started or used a tool: its work, not the turn's. A background agent
    /// works on long after the turn that started it is over.
    AgentAtWork(String),
    /// A subagent's own permission prompt or question is up.
    AgentWaiting(String),
    /// `SubagentStop`.
    AgentDone(String),
}

/// Claude Code's word for why a turn failed, as a static string. Anything it
/// adds later reads as `unknown` until it is listed here.
pub fn api_error_kind(s: &str) -> &'static str {
    const KINDS: [&str; 13] = [
        "authentication_failed",
        "oauth_org_not_allowed",
        "account_on_hold",
        "verification_required",
        "billing_error",
        "rate_limit",
        "overloaded",
        "invalid_request",
        "model_not_found",
        "server_error",
        "max_output_tokens",
        "cloud_credential_error",
        "unknown",
    ];
    KINDS.iter().copied().find(|k| *k == s).unwrap_or("unknown")
}

/// How many a list in a hook's input names: the items of an array, the keys
/// of an object, a count as given; nothing for anything else.
pub fn listed(v: &serde_json::Value) -> u32 {
    match v {
        serde_json::Value::Array(a) => a.len() as u32,
        serde_json::Value::Object(o) => o.len() as u32,
        serde_json::Value::Number(n) => n.as_u64().unwrap_or(0).min(u32::MAX as u64) as u32,
        serde_json::Value::Bool(true) => 1,
        _ => 0,
    }
}

impl HookEvent {
    pub fn parse(body: &str) -> Option<HookEvent> {
        let v: serde_json::Value = serde_json::from_str(body).ok()?;
        let asks = |v: &serde_json::Value| match v["hook_event_name"].as_str() {
            Some("PermissionRequest") => true,
            Some("PreToolUse") => v["tool_name"] == "AskUserQuestion",
            Some("Notification") => matches!(v["notification_type"].as_str(), Some("permission_prompt" | "elicitation_dialog")),
            _ => false,
        };
        // Only a subagent's hooks carry its id, and none of them is the turn's:
        // the turn that started a background agent may long be over.
        if let Some(agent) = v["agent_id"].as_str().filter(|a| !a.is_empty()) {
            let agent = agent.to_string();
            return match v["hook_event_name"].as_str()? {
                "SubagentStop" => Some(HookEvent::AgentDone(agent)),
                _ if asks(&v) => Some(HookEvent::AgentWaiting(agent)),
                "Notification" | "PreToolUse" => None,
                _ => Some(HookEvent::AgentAtWork(agent)),
            };
        }
        match v["hook_event_name"].as_str()? {
            "UserPromptSubmit" => Some(HookEvent::Submitted),
            "PostToolUse" | "PostToolUseFailure" => Some(HookEvent::Busy),
            "Stop" => match (listed(&v["background_tasks"]), listed(&v["session_crons"])) {
                (0, 0) => Some(HookEvent::Idle),
                (tasks, crons) => Some(HookEvent::IdleWith { tasks, crons }),
            },
            // A permission prompt is up, or a question the model put.
            "PermissionRequest" => Some(HookEvent::Waiting),
            "PreToolUse" if v["tool_name"] == "AskUserQuestion" => Some(HookEvent::Waiting),
            "StopFailure" => Some(HookEvent::Failed(api_error_kind(v["error"].as_str().unwrap_or("")))),
            "SessionEnd" => Some(HookEvent::Gone),
            "Interrupt" => Some(HookEvent::Interrupted),
            "Notification" => match v["notification_type"].as_str().unwrap_or("") {
                "permission_prompt" | "elicitation_dialog" => Some(HookEvent::Waiting),
                // `idle_prompt` (waiting for the next prompt), `auth_success`:
                // nothing to act on, but a well-formed event.
                _ => None,
            },
            // PreToolUse and the rest: the turn is already known to be busy.
            _ => None,
        }
    }
}

impl HookEvent {
    /// The command a tool's hook is about (`tool_input.command`), where it
    /// names one: what a Codex permission is for, and what answers it.
    pub fn subject(body: &str) -> Option<String> {
        let v: serde_json::Value = serde_json::from_str(body).ok()?;
        v["tool_input"]["command"].as_str().map(str::to_owned)
    }
}

/// A hook, about no command, as the tests hand one over (`on_hook_about`).
#[cfg(test)]
pub fn on_hook(chat_id: &str, ev: HookEvent) {
    on_hook_about(chat_id, ev, None);
}

/// A hook arrived for a session (hub.rs), with the command it is about
/// (`HookEvent::subject`): its record changes at once; the timestamp is the
/// truth for "when did the turn start".
pub fn on_hook_about(chat_id: &str, ev: HookEvent, subject: Option<String>) {
    let now = now_ms();
    // Which CLI's: a Codex chat's turns are counted off its rollout. Asked
    // before the lock, as a read does.
    let provider = crate::pty::manager().provider(chat_id);
    // The first prompt, if no submit into its pane was seen to go in first:
    // what its job holds now came up with its CLI. Only then: by a later hook
    // (a `PostToolUse` with the prompt's own hook missed) a tool may have
    // started a shell, which is no furniture.
    if ev == HookEvent::Submitted {
        submitting(chat_id);
    }
    let starts = matches!(ev, HookEvent::Busy | HookEvent::Submitted | HookEvent::Waiting);
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    let st = &mut *st;
    let t = track(&mut st.tracks, &mut st.ended, chat_id);
    if let Some(p) = provider {
        t.codex = p == Provider::Codex;
    }
    if starts && !t.turned {
        t.first_turn_ms.get_or_insert(now);
        t.turned = true;
    }
    if t.codex {
        hear_codex(t, &ev, subject, now);
    }
    apply(t, ev, now);
    refold(t, now);
    // A hook is the CLI at work, whatever it says: a turn that began and ended
    // between two reads starts a power-off's quiet window over all the same.
    stamp(st);
}

/// A prompt is about to go into a terminal session's CLI: a person's Enter on
/// a draft, Luna's text sent, or the CLI's own `UserPromptSubmit`, whichever
/// comes first. Its first one for a CLI process takes the last look at the
/// CLI's own furniture (its MCP servers) in its job, before any turn could
/// start a shell there; none after adds to it (`Track::furnished_for`). Takes
/// the session map and lists the job: not to be called with either held.
pub fn submitting(chat_id: &str) {
    let Some(p) = crate::pty::manager().probe(chat_id) else { return };
    let done = |t: &Track| furnished(t, p.spawned_at_ms);
    if state().lock().unwrap_or_else(|e| e.into_inner()).tracks.get(chat_id).is_some_and(done) {
        return;
    }
    // Read before the lock, as a read of the job takes a moment and a hook's
    // curl waits.
    let before = crate::procs::in_job(p.job.as_deref(), p.pid);
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    let st = &mut *st;
    furnish(track(&mut st.tracks, &mut st.ended, chat_id), p.spawned_at_ms, before);
}

/// Whether the furniture of the CLI process spawned at `spawned_at_ms` is
/// taken: its first prompt went in, or a turn of it began.
pub(super) fn furnished(t: &Track, spawned_at_ms: u128) -> bool {
    t.furnished_for == spawned_at_ms || (t.turned && t.spawned_at_ms == spawned_at_ms)
}

/// The furniture of the CLI process spawned at `spawned_at_ms`, as its first
/// prompt goes in: what its job holds then, on top of what the looks before
/// found. A job that will not list adds nothing, and nothing is added after.
pub(super) fn furnish(t: &mut Track, spawned_at_ms: u128, job: Option<Vec<(crate::procs::ProcRead, String)>>) {
    if furnished(t, spawned_at_ms) {
        return;
    }
    // A process no read has folded yet: what was furniture was the last one's.
    if t.spawned_at_ms != 0 && t.spawned_at_ms != spawned_at_ms {
        t.furniture.clear();
    }
    t.furniture.extend(job.into_iter().flatten().map(|(r, _)| r));
    t.furnished_for = spawned_at_ms;
}

/// What a Codex chat's hook says beyond the turn's word (`fold_codex`): its
/// hooks deliver, and a permission is up for `subject`, or that one answered
/// (its command's `PostToolUse`), or none is left (the next prompt, the
/// session's end). A parallel tool's hook answers nothing.
pub(super) fn hear_codex(t: &mut Track, ev: &HookEvent, subject: Option<String>, now: u64) {
    t.heard_ms = now;
    match ev {
        HookEvent::Waiting => t.asked = Some((subject.unwrap_or_default(), now)),
        HookEvent::Busy if t.asked.as_ref().is_some_and(|(asked, _)| !asked.is_empty() && subject.as_deref() == Some(asked.as_str())) => {
            t.asked = None
        }
        HookEvent::Submitted | HookEvent::Gone => t.asked = None,
        _ => {}
    }
}

/// What one hook changes of a track.
pub(super) fn apply(t: &mut Track, ev: HookEvent, now: u64) {
    // A subagent's word: the turn, its start and its end are not its to say,
    // but it is the session's latest word, which an older registry idle does
    // not overrule (`fold_claude`).
    match ev {
        HookEvent::AgentAtWork(agent) => {
            t.agents.insert(agent, false);
            t.hook_at_ms = t.hook_at_ms.max(now);
            return;
        }
        HookEvent::AgentWaiting(agent) => {
            t.agents.insert(agent, true);
            t.hook_at_ms = t.hook_at_ms.max(now);
            return;
        }
        HookEvent::AgentDone(agent) => {
            // The registry may go on saying busy: the turn Claude Code starts
            // on the agent's result. Judged afresh from here (STALE_DISK_MS),
            // not by a busy that was the agent's. Not for an agent never
            // seen at work (one of the CLI's own): its busy was none of ours.
            if t.agents.remove(&agent).is_some() {
                t.disk_busy_since_ms = None;
            }
            return;
        }
        _ => {}
    }
    t.failed = None;
    t.api_error = None;
    match ev {
        HookEvent::Busy | HookEvent::Submitted | HookEvent::Waiting => {
            t.hook = Some(if ev == HookEvent::Waiting { Turn::Waiting } else { Turn::Busy });
            t.ended_by_hook = false;
            t.read_counted = false;
        }
        HookEvent::Idle | HookEvent::IdleWith { .. } => {
            (t.stop_tasks, t.stop_crons) = match ev {
                HookEvent::IdleWith { tasks, crons } => (tasks, crons),
                _ => (0, 0),
            };
            t.hook = Some(Turn::Idle);
            // `Stop`: a turn is over, whether or not any read saw it run;
            // counted once, a read having counted it already when it is late.
            count_stop(t);
            t.was_busy = false;
            t.ended_by_hook = true;
        }
        HookEvent::Failed(error) => {
            t.hook = Some(Turn::Idle);
            count_stop(t);
            t.was_busy = false;
            t.ended_by_hook = true;
            t.failed = Some(error);
        }
        HookEvent::Interrupted => {
            t.hook = Some(Turn::Idle);
            count_stop(t);
            t.was_busy = false;
            t.ended_by_hook = true;
            t.failed = Some("interrupted");
        }
        HookEvent::Gone => {
            t.hook = Some(Turn::Idle);
            t.agents.clear();
            (t.stop_tasks, t.stop_crons) = (0, 0);
        }
        HookEvent::AgentAtWork(_) | HookEvent::AgentWaiting(_) | HookEvent::AgentDone(_) => return,
    }
    t.hook_at_ms = now;
}

/// A `Stop` or `StopFailure` counts its turn, unless a read counted it as
/// ended when that hook was missed (reconcile.rs `take`) and no turn has begun
/// since: then it is that turn's, late. Not Codex's: its rollout counts its
/// turns (`count_on`), an aborted one too, and a `Stop` a hook of the
/// account's own blocked comes again within the same turn.
fn count_stop(t: &mut Track) {
    if t.codex {
        return;
    }
    if !std::mem::take(&mut t.read_counted) {
        t.turns_ended += 1;
    }
}

/// The hook's last word on a session, if any, for the tests of what a hook
/// reaching the hub does.
#[cfg(test)]
pub fn hook_turn(chat_id: &str) -> Option<Turn> {
    state().lock().unwrap_or_else(|e| e.into_inner()).tracks.get(chat_id).and_then(|t| t.hook)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hook_events() {
        let ev = |name: &str, extra: &str| HookEvent::parse(&format!(r#"{{"session_id":"s","hook_event_name":"{name}"{extra}}}"#));
        assert_eq!(ev("UserPromptSubmit", ""), Some(HookEvent::Submitted));
        assert_eq!(ev("PostToolUse", r#","tool_name":"Bash""#), Some(HookEvent::Busy));
        assert_eq!(ev("Stop", r#","stop_hook_active":false"#), Some(HookEvent::Idle));
        // Claude Code 2.1.327's `Stop` lists what runs on beside the turn.
        assert_eq!(ev("Stop", r#","background_tasks":[],"session_crons":[]"#), Some(HookEvent::Idle));
        assert_eq!(
            ev("Stop", r#","background_tasks":[{"task_id":"b1","task_type":"local_bash"},{"task_id":"a2"}],"session_crons":[{"id":"c1"}]"#),
            Some(HookEvent::IdleWith { tasks: 2, crons: 1 })
        );
        assert_eq!(ev("Stop", r#","background_tasks":3"#), Some(HookEvent::IdleWith { tasks: 3, crons: 0 }), "a count as given");
        assert_eq!(ev("SessionEnd", ""), Some(HookEvent::Gone));
        assert_eq!(ev("Notification", r#","notification_type":"permission_prompt""#), Some(HookEvent::Waiting));
        assert_eq!(ev("Notification", r#","notification_type":"idle_prompt""#), None);
        assert_eq!(ev("PreToolUse", ""), None);
        assert_eq!(ev("PreToolUse", r#","tool_name":"AskUserQuestion""#), Some(HookEvent::Waiting));
        assert_eq!(ev("PermissionRequest", r#","tool_name":"Bash""#), Some(HookEvent::Waiting));
        assert_eq!(HookEvent::parse("not json"), None);
        assert_eq!(ev("StopFailure", r#","error":"rate_limit""#), Some(HookEvent::Failed("rate_limit")));
        assert_eq!(ev("StopFailure", r#","error":"something_new""#), Some(HookEvent::Failed("unknown")));
        // A subagent's hooks, by the id only they carry, are never the turn's.
        let agent = |name: &str, extra: &str| ev(name, &format!(r#","agent_id":"a1"{extra}"#));
        let a1 = || "a1".to_string();
        assert_eq!(agent("SubagentStart", r#","agent_type":"general-purpose""#), Some(HookEvent::AgentAtWork(a1())));
        assert_eq!(agent("PostToolUse", r#","tool_name":"Bash""#), Some(HookEvent::AgentAtWork(a1())));
        assert_eq!(agent("PermissionRequest", r#","tool_name":"Bash""#), Some(HookEvent::AgentWaiting(a1())));
        assert_eq!(agent("Notification", r#","notification_type":"permission_prompt""#), Some(HookEvent::AgentWaiting(a1())));
        assert_eq!(agent("Notification", r#","notification_type":"idle_prompt""#), None);
        assert_eq!(agent("SubagentStop", ""), Some(HookEvent::AgentDone(a1())));
        assert_eq!(ev("SubagentStop", ""), None, "nobody's to count");
    }

    /// Codex 0.169.0's hooks as they reached a listener on 2026-11-20 (a chat's
    /// over `codex app-server` on a clone, the user's name taken out of the
    /// paths): the same names and fields as Claude Code's, `Interrupt` its own,
    /// and no `agent_id` on the root thread's.
    #[test]
    fn parses_codex_hook_events() {
        let head = r#""session_id":"01a1f62b-b476-7391-96b5-d0e8a2646483","turn_id":"01a1f62c-81c2-7150-b50c-994b128c8611","transcript_path":"E:\\t\\rollout-2026-11-20T06-37-01-01a1f62b-b476-7391-96b5-d0e8a2646483.jsonl","cwd":"C:\\Users\\user\\work""#;
        let ev = |rest: &str| HookEvent::parse(&format!("{{{head},{rest}}}"));
        assert_eq!(
            ev(r#""hook_event_name":"UserPromptSubmit","model":"gpt-6-luna","permission_mode":"bypassPermissions","prompt":"Run it""#),
            Some(HookEvent::Submitted)
        );
        assert_eq!(
            ev(
                r#""hook_event_name":"PermissionRequest","model":"gpt-6-luna","permission_mode":"default","tool_name":"Bash","tool_input":{"command":"Set-Content -LiteralPath probe.txt -Value hi"}"#
            ),
            Some(HookEvent::Waiting)
        );
        assert_eq!(
            ev(
                r#""hook_event_name":"PostToolUse","model":"gpt-6-luna","permission_mode":"default","tool_name":"Bash","tool_input":{"command":"ping -n 8 127.0.0.1"},"tool_response":"Pinging""#
            ),
            Some(HookEvent::Busy)
        );
        assert_eq!(
            ev(
                r#""hook_event_name":"Stop","model":"gpt-6-luna","permission_mode":"default","stop_hook_active":false,"last_assistant_message":"DONE""#
            ),
            Some(HookEvent::Idle)
        );
        assert_eq!(
            ev(r#""hook_event_name":"Interrupt","model":"gpt-6-luna","permission_mode":"bypassPermissions""#),
            Some(HookEvent::Interrupted)
        );
        assert_eq!(ev(r#""hook_event_name":"SessionEnd","reason":"other""#), Some(HookEvent::Gone));
        // What a permission and a tool's hook are about.
        let about = |rest: &str| HookEvent::subject(&format!("{{{head},{rest}}}"));
        assert_eq!(
            about(r#""hook_event_name":"PermissionRequest","tool_name":"Bash","tool_input":{"command":"Set-Content -LiteralPath probe.txt -Value hi"}"#)
                .as_deref(),
            Some("Set-Content -LiteralPath probe.txt -Value hi")
        );
        assert_eq!(about(r#""hook_event_name":"Stop","stop_hook_active":false"#), None);
        // A subagent's, by its thread id (0.169.0 `subagent_hook_context`).
        let sub = |rest: &str| ev(&format!(r#""agent_id":"01a1f700-0000-7000-8000-000000000001","agent_type":"default",{rest}"#));
        let a = || "01a1f700-0000-7000-8000-000000000001".to_string();
        assert_eq!(sub(r#""hook_event_name":"SubagentStart""#), Some(HookEvent::AgentAtWork(a())));
        assert_eq!(sub(r#""hook_event_name":"PermissionRequest","tool_name":"Bash""#), Some(HookEvent::AgentWaiting(a())));
        assert_eq!(sub(r#""hook_event_name":"SubagentStop","last_assistant_message":"done""#), Some(HookEvent::AgentDone(a())));
    }

    /// A Codex chat's turns are counted off its rollout: its `Stop` and
    /// `Interrupt` count nothing.
    #[test]
    fn a_codex_hook_counts_no_turn() {
        let mut t = Track { codex: true, ..Default::default() };
        apply(&mut t, HookEvent::Submitted, 1_000);
        apply(&mut t, HookEvent::Idle, 2_000);
        apply(&mut t, HookEvent::Submitted, 4_000);
        apply(&mut t, HookEvent::Interrupted, 5_000);
        assert_eq!(t.turns_ended, 0);
        // Claude Code's `Stop` counts as ever.
        let mut t = Track::default();
        apply(&mut t, HookEvent::Submitted, 1_000);
        apply(&mut t, HookEvent::Idle, 2_000);
        assert_eq!(t.turns_ended, 1);
    }

    /// As on 2026-11-04: the turn ends having started a background agent,
    /// whose tools then fire hooks for an hour while the registry says busy.
    /// The turn is over, the session's job is not.
    #[test]
    fn a_background_agent_is_no_turn() {
        let id = "bg-agent";
        let hook = |body: &str| on_hook(id, HookEvent::parse(body).unwrap());
        hook(r#"{"hook_event_name":"UserPromptSubmit","prompt":"review it"}"#);
        hook(r#"{"hook_event_name":"SubagentStart","agent_id":"a1","agent_type":"general-purpose"}"#);
        hook(r#"{"hook_event_name":"PostToolUse","tool_name":"Agent"}"#);
        hook(r#"{"hook_event_name":"Stop"}"#);
        hook(r#"{"hook_event_name":"PostToolUse","tool_name":"Bash","agent_id":"a1"}"#);
        // The registry, read at `at`, says busy.
        let read = |at: u64| {
            let mut st = state().lock().unwrap();
            let t = st.tracks.get_mut(id).unwrap();
            t.disk = Some(Turn::Busy);
            t.disk_busy_since_ms.get_or_insert(at);
            refold(t, at);
            t.life.clone()
        };
        {
            let st = state().lock().unwrap();
            let t = &st.tracks[id];
            assert_eq!((t.hook, t.turns_ended, t.agents.len()), (Some(Turn::Idle), 1, 1), "its tool spoke for itself");
        }
        let l = read(now_ms());
        assert_eq!(l.phase, Phase::Ended, "a busy registry is the agent's, not a turn");
        assert_eq!(l.background.subagents, 1);
        let ts = turn_state(id).unwrap();
        assert!(ts.turn == Turn::Idle && ts.children_busy, "between turns, with a job at work");
        assert!(in_background(id) && l.active(), "which holds the machine");

        // Its own prompt holds the session on it, turn or no turn.
        hook(r#"{"hook_event_name":"PermissionRequest","tool_name":"Bash","agent_id":"a1"}"#);
        let l = read(now_ms());
        assert_eq!((l.phase, l.background.subagents), (Phase::Asking, 0));
        hook(r#"{"hook_event_name":"PostToolUse","tool_name":"Bash","agent_id":"a1"}"#);

        // One of the CLI's own agents, never seen at work, ends: nothing of
        // the session's starts over for it.
        let mut st = state().lock().unwrap();
        st.tracks.get_mut(id).unwrap().disk_busy_since_ms = Some(1);
        drop(st);
        hook(r#"{"hook_event_name":"SubagentStop","agent_id":"internal"}"#);
        assert_eq!(state().lock().unwrap().tracks[id].disk_busy_since_ms, Some(1));

        // It ends; Claude Code takes its result up in a turn of its own,
        // which the registry says before any hook may: busy, judged afresh.
        hook(r#"{"hook_event_name":"SubagentStop","agent_id":"a1"}"#);
        let at = now_ms();
        assert_eq!(read(at).phase, Phase::Running);
        assert_eq!(read(at + STALE_DISK_MS + 1).phase, Phase::Ended, "no hook since: left behind");
        assert!(!turn_state(id).unwrap().children_busy);
        forget(id);
    }

    #[test]
    fn a_turn_s_start_lasts_as_long_as_the_turn() {
        let id = "chat-turn-start";
        assert_eq!(turn_started_ms(id), None);
        on_hook(id, HookEvent::Busy);
        let at = turn_started_ms(id).expect("begun");
        state().lock().unwrap().tracks.get_mut(id).unwrap().turn_started_ms = Some(at - 1000);
        on_hook(id, HookEvent::Busy);
        on_hook(id, HookEvent::Waiting);
        assert_eq!(turn_started_ms(id), Some(at - 1000), "a tool call or a prompt inside the turn does not move it");
        on_hook(id, HookEvent::Idle);
        assert_eq!(turn_started_ms(id), None, "null between turns");
        on_hook(id, HookEvent::Busy);
        on_hook(id, HookEvent::Failed("rate_limit"));
        assert_eq!(turn_started_ms(id), None);
        forget(id);
    }

    #[test]
    fn a_failed_turn_is_over_and_remembers_why() {
        on_hook("chat-limit", HookEvent::Busy);
        on_hook("chat-limit", HookEvent::Failed("rate_limit"));
        {
            let st = state().lock().unwrap();
            let t = st.tracks.get("chat-limit").unwrap();
            assert_eq!((t.hook, t.failed, t.turns_ended), (Some(Turn::Idle), Some("rate_limit"), 1));
        }
        // The next prompt is a new turn, and the error is behind it.
        on_hook("chat-limit", HookEvent::Busy);
        assert_eq!(state().lock().unwrap().tracks.get("chat-limit").unwrap().failed, None);
    }

    #[test]
    fn a_stop_hook_ends_a_turn_no_sample_saw() {
        on_hook("chat-fast-turn", HookEvent::Busy);
        on_hook("chat-fast-turn", HookEvent::Idle);
        on_hook("chat-fast-turn", HookEvent::Busy);
        on_hook("chat-fast-turn", HookEvent::Idle);
        assert_eq!(turn_state("chat-fast-turn").map(|t| t.turns_ended), Some(2));
        // SessionEnd is not a turn.
        on_hook("chat-fast-turn", HookEvent::Gone);
        assert_eq!(turn_state("chat-fast-turn").map(|t| t.turns_ended), Some(2));
    }
}
