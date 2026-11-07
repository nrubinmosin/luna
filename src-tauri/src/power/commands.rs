//! The window's power commands, and `luna_power` arming a rule for an agent.

use super::*;

#[tauri::command]
pub fn power_state() -> PowerState {
    current()
}

#[tauri::command]
pub fn set_keep_awake(on: bool) -> Result<PowerState, String> {
    crate::settings::update(|s| s.keep_awake = on)?;
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    if !on && st.holding && st.armed.is_none() {
        os::hold(false);
        st.holding = false;
        st.release_at_ms = None;
    }
    publish(&mut st, &crate::activity::summary());
    Ok(snapshot(&st, &crate::activity::summary()))
}

#[tauri::command]
pub fn arm_power_off(action: String, quiet_s: u64, drain: Option<bool>) -> Result<PowerState, String> {
    let action = Action::parse(&action).ok_or_else(|| format!("unknown action {action}"))?;
    if let Some(why) = unsupported(action, cfg!(windows)) {
        return Err(why);
    }
    let quiet_s = quiet_s.clamp(30, 6 * 3600);
    crate::settings::update(|s| {
        s.power_action = action.as_str().to_string();
        s.quiet_window_s = quiet_s;
    })?;
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    // Left unsaid it stands: a second agent's arm does not lift a drain by the way.
    let drain = drain.unwrap_or_else(|| st.armed.as_ref().is_some_and(|a| a.drain));
    st.armed = Some(Armed { action, quiet_s, armed_at_ms: now_ms(), drain });
    st.countdown_ends_at_ms = None;
    st.blocked_why.clear();
    st.blocked_logged_ms = 0;
    let summary = crate::activity::summary();
    let (waiting, stalled) = parked(&summary);
    let mut line = format!("armed: {} after {quiet_s}s quiet", action.as_str());
    if drain {
        line.push_str(", draining");
    }
    if dry() {
        line.push_str(" (dry run: this build only logs what it would do)");
    }
    if !waiting.is_empty() {
        line.push_str(&format!("; waiting for a person: {}", waiting.join(", ")));
    }
    if !stalled.is_empty() {
        line.push_str(&format!("; stopped on an API error, counted as done: {}", stalled.join(", ")));
    }
    crate::log::warn("power", &line);
    // Said at once, not found out in the morning: the menu closes on Arm,
    // and nothing else would tell that the rule cannot fire as things stand.
    if !waiting.is_empty() {
        notify(&format!("{} waiting for you — Luna will not {} until answered", plural(waiting.len()), verb(action)), &waiting.join(", "));
    }
    publish(&mut st, &summary);
    if drain {
        let (held, left) = settle(st, &ENTERED_ENDED, SETTLE_WAIT);
        st = held;
        if left > 0 {
            crate::log::warn("power", &format!("{left} call(s) let in before the drain still run; the rule waits for them"));
        }
    }
    Ok(snapshot(&st, &crate::activity::summary()))
}

#[tauri::command]
pub fn disarm_power_off() -> PowerState {
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    if st.armed.take().is_some() {
        crate::log::warn("power", "disarmed");
    }
    st.countdown_ends_at_ms = None;
    publish(&mut st, &crate::activity::summary());
    snapshot(&st, &crate::activity::summary())
}

/// A rule as `luna_power` gives it: as it stands now, which is none if somebody
/// disarmed it while a drain's arming waited.
fn armed_as_told(armed: Option<&Armed>) -> serde_json::Value {
    armed.map_or(serde_json::Value::Null, |a| serde_json::json!({ "action": a.action.as_str(), "quietS": a.quiet_s, "drain": a.drain }))
}

/// An agent's `luna_power`: the rule the power chip arms, so the PC goes down
/// the way Luna takes it down — once every session is quiet, each CLI asked to
/// quit first — rather than by a `shutdown` that takes the other chats' work
/// with it. "off" disarms. Says what still holds the rule up, the caller aside:
/// its own turn ends once it has answered, and whether this is a dry run (`dry`),
/// for a test to see before any countdown can end.
pub fn arm_for_agent(caller: &str, action: &str, quiet_s: Option<u64>, drain: Option<bool>) -> Result<serde_json::Value, String> {
    if action == "off" {
        disarm_power_off();
        return Ok(serde_json::json!({ "armed": null }));
    }
    let Some(a) = Action::parse(action).filter(|a| *a != Action::Log) else {
        return Err(format!("unknown action {action:?}; use shutdown, restart, hibernate, sleep or off"));
    };
    let quiet_s = quiet_s.unwrap_or(crate::settings::get().quiet_window_s);
    let state = arm_power_off(a.as_str().to_string(), quiet_s, drain)?;
    let name = |id: &str| crate::agents::chat_name(id).unwrap_or_else(|| id.to_string());
    let busy: Vec<String> = state.summary.sessions.iter().filter(|s| s.busy && s.id != caller).map(|s| name(&s.id)).collect();
    let (waiting, _) = parked(&state.summary);
    let mut out = serde_json::json!({
        "armed": armed_as_told(state.armed.as_ref()),
        "busy": busy,
        "waitingForAPerson": waiting,
    });
    if dry() {
        out["dryRun"] = serde_json::json!(true);
    }
    Ok(out)
}

/// Let go of the awake request on the way out.
pub fn clear() {
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    if st.holding {
        os::hold(false);
        st.holding = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn luna_power_gives_the_rule_as_it_stands_after_settling() {
        assert_eq!(armed_as_told(None), serde_json::Value::Null, "disarmed while it waited");
        let rule = Armed { action: Action::Restart, quiet_s: 60, armed_at_ms: 0, drain: true };
        assert_eq!(armed_as_told(Some(&rule)), serde_json::json!({ "action": "restart", "quietS": 60, "drain": true }));
    }

    #[test]
    fn an_agent_arms_only_what_the_chip_offers() {
        for bad in ["reboot", "log", ""] {
            let e = arm_for_agent("me", bad, None, None).unwrap_err();
            assert!(e.contains("use shutdown, restart, hibernate, sleep or off"), "{e}");
        }
        assert!(state().lock().unwrap().armed.is_none(), "nothing armed on a refusal");
    }
}
