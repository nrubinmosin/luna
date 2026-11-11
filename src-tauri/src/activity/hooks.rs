//! What a Claude Code hook says of a session (hub.rs hands each one over): the
//! turn begun, waiting on a person, over or failed, and a subagent's word apart
//! from the turn's.

use super::*;

/// What a Claude Code hook told us.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HookEvent {
    /// `UserPromptSubmit`, `PostToolUse`: the model is at work.
    Busy,
    /// `Notification` with `permission_prompt` or `elicitation_dialog`.
    Waiting,
    /// `Stop`: the turn is over.
    Idle,
    /// `StopFailure`: the turn is over because the API refused it — a usage
    /// limit, a lost login. No `Stop` follows.
    Failed(&'static str),
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
            "UserPromptSubmit" | "PostToolUse" | "PostToolUseFailure" => Some(HookEvent::Busy),
            "Stop" => Some(HookEvent::Idle),
            // A permission prompt is up, or a question the model put.
            "PermissionRequest" => Some(HookEvent::Waiting),
            "PreToolUse" if v["tool_name"] == "AskUserQuestion" => Some(HookEvent::Waiting),
            "StopFailure" => Some(HookEvent::Failed(api_error_kind(v["error"].as_str().unwrap_or("")))),
            "SessionEnd" => Some(HookEvent::Gone),
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

/// A hook arrived for a session (hub.rs). Takes effect at the next sample;
/// the timestamp is the truth for "when did the turn start".
pub fn on_hook(chat_id: &str, ev: HookEvent) {
    let now = now_ms();
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    let st = &mut *st;
    let t = track(&mut st.tracks, &mut st.ended, chat_id);
    // A subagent's word: the turn, its start and its end are not its to say.
    match ev {
        HookEvent::AgentAtWork(agent) => {
            t.agents.insert(agent, false);
            return;
        }
        HookEvent::AgentWaiting(agent) => {
            t.agents.insert(agent, true);
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
    match ev {
        HookEvent::Busy => {
            if t.first_turn_ms == 0 {
                t.first_turn_ms = now;
            }
            t.turn_started_ms.get_or_insert(now);
            t.hook = Some(Turn::Busy);
        }
        HookEvent::Waiting => {
            t.turn_started_ms.get_or_insert(now);
            t.hook = Some(Turn::Waiting);
        }
        HookEvent::Idle => {
            t.hook = Some(Turn::Idle);
            t.turn_started_ms = None;
            // `Stop`: a turn is over, whether or not any sample saw it run.
            t.turns_ended += 1;
            t.was_busy = false;
        }
        HookEvent::Failed(error) => {
            t.hook = Some(Turn::Idle);
            t.turn_started_ms = None;
            t.turns_ended += 1;
            t.was_busy = false;
            t.failed = Some(error);
        }
        HookEvent::Gone => {
            t.hook = Some(Turn::Idle);
            t.turn_started_ms = None;
            t.agents.clear();
        }
        HookEvent::AgentAtWork(_) | HookEvent::AgentWaiting(_) | HookEvent::AgentDone(_) => return,
    }
    t.hook_at_ms = now;
}

/// The hook's last word on a session, if any: newer than the last sample's
/// verdict, as a hook takes effect here at once.
pub fn hook_turn(chat_id: &str) -> Option<Turn> {
    state().lock().unwrap_or_else(|e| e.into_inner()).tracks.get(chat_id).and_then(|t| t.hook)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hook_events() {
        let ev = |name: &str, extra: &str| HookEvent::parse(&format!(r#"{{"session_id":"s","hook_event_name":"{name}"{extra}}}"#));
        assert_eq!(ev("UserPromptSubmit", ""), Some(HookEvent::Busy));
        assert_eq!(ev("PostToolUse", r#","tool_name":"Bash""#), Some(HookEvent::Busy));
        assert_eq!(ev("Stop", r#","stop_hook_active":false"#), Some(HookEvent::Idle));
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
        let mut st = state().lock().unwrap();
        let t = st.tracks.get_mut(id).unwrap();
        assert_eq!((t.hook, t.turns_ended, t.agents.len()), (Some(Turn::Idle), 1, 1), "its tool spoke for itself");
        t.turn = combine(t, Some(Turn::Busy), now_ms());
        assert_eq!(t.turn, Turn::Idle, "a busy registry is the agent's, not a turn");
        assert_eq!(agents_at_work(t).as_deref(), Some("1 subagent"));
        drop(st);
        let ts = turn_state(id).unwrap();
        assert!(ts.turn == Turn::Idle && ts.children_busy, "between turns, with a job at work");
        assert!(children_hold(id), "which holds the machine");
        assert_eq!(left_quiet(id), None, "and is not to be called idle");

        // Its own prompt holds the session on it, turn or no turn.
        hook(r#"{"hook_event_name":"PermissionRequest","tool_name":"Bash","agent_id":"a1"}"#);
        let mut st = state().lock().unwrap();
        let t = st.tracks.get_mut(id).unwrap();
        assert_eq!(combine(t, Some(Turn::Busy), now_ms()), Turn::Waiting);
        assert_eq!(agents_at_work(t), None, "waiting for a person keeps no machine awake");
        drop(st);
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
        let mut st = state().lock().unwrap();
        let t = st.tracks.get_mut(id).unwrap();
        let at = now_ms();
        assert_eq!(combine(t, Some(Turn::Busy), at), Turn::Busy);
        assert_eq!(combine(t, Some(Turn::Busy), at + STALE_DISK_MS + 1), Turn::Idle, "no hook since: left behind");
        drop(st);
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
