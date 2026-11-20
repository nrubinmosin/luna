//! The status bar's screenshot scenes (the harness in shots.rs): the bar in
//! its states over the fixture's chats and the demo's accounts, the panels,
//! and the orphan line. The clock stands still at `NOW_MS`, so the countdowns
//! on them read the same every run.

use super::{Popover, Vitals, cmd, event};
use crate::demo::{self, Scene};
use crate::desktop;
use crate::dto::{CoreState, LinkState, ThemePref};
use crate::host::cmd as host_cmd;
use serde_json::{Value, json};
use std::rc::Rc;
use std::time::Duration;

/// 2026-11-19T12:00:00Z: the instant every demo reads.
pub const NOW_MS: u64 = 1_795_089_600_000;

/// What a shot holds still: the clock, and the pause between the accounts'
/// asks (a shot is taken a moment after the window opens).
#[cfg_attr(not(feature = "shots"), allow(dead_code))]
pub fn hold(v: &mut Vitals) {
    v.set_clock(Rc::new(|| NOW_MS));
    v.set_gap(Duration::ZERO);
}

fn claude() -> Value {
    json!({
        "signedIn": true, "plan": "Max", "haveUsage": true, "h5": 0.42, "week": 0.17,
        "resetH5": "2026-11-19T15:05:00Z", "resetWeek": "2026-11-24T12:00:00Z",
        "scoped": [{ "kind": "weekly", "label": "Fable", "used": 0.91, "resetsAt": "2026-11-24T12:00:00Z" }], "other": [],
        "fetchedAtMs": NOW_MS as f64, "stale": false, "refreshError": null, "rateLimited": null
    })
}

fn codex() -> Value {
    json!({
        "signedIn": true, "plan": "Pro", "haveUsage": true,
        "windows": [
            { "id": "primary", "label": "5 hours", "used": 0.66, "resetsAt": "2026-11-19T12:40:00Z" },
            { "id": "secondary", "label": "week", "used": 0.3, "resetsAt": "2026-11-22T12:00:00Z" }
        ],
        "fetchedAtMs": NOW_MS as f64, "stale": false, "rateLimited": null
    })
}

fn offer(core: &str) -> Value {
    json!({ "current": "0.20.0", "offer": {
        "version": "0.20.1",
        "notes": "- The status bar: counts, accounts with usage meters, the update, the CLIs and the power rule.\n- Fixes the orphan line's wording.",
        "pubDate": null, "coreFingerprint": core
    } })
}

/// A power state with the sessions given (id, turn, busy) and the rule.
fn power(holding: bool, armed: Option<(&str, u64, bool)>, ends: Option<u64>, waiting: usize, sessions: &[(&str, &str, bool)]) -> Value {
    json!({
        "keepAwake": true, "holding": holding,
        "armed": armed.map(|(action, quiet, drain)| json!({ "action": action, "quietS": quiet, "armedAtMs": NOW_MS - 600_000, "drain": drain })),
        "countdownEndsAtMs": ends,
        "summary": { "busy": sessions.iter().filter(|s| s.2).count(), "waiting": waiting, "stalled": 0, "idleSinceMs": null,
            "sessions": sessions.iter().map(|(id, turn, busy)| json!({
                "id": id, "turn": turn, "procs": [], "outputFresh": false, "busy": busy, "stalled": null
            })).collect::<Vec<_>>() }
    })
}

fn orphans() -> Value {
    json!([
        { "id": "s-lost-1", "provider": "claude", "pid": 5120, "cwd": "C:\\work\\parser", "accountPath": "C:\\accounts\\anthropic\\work", "title": "Parser fix", "status": "busy" },
        { "id": "s-lost-2", "provider": "codex", "pid": 6044, "cwd": "C:\\work\\site", "accountPath": "C:\\accounts\\openai\\personal", "title": null, "status": null }
    ])
}

fn base(name: &'static str, theme: ThemePref) -> Scene {
    let up = CoreState { state: LinkState::Up, core: Some(demo::core()), error: None, revision: 1 };
    Scene {
        name,
        theme,
        core: up,
        then: Vec::new(),
        terminal: None,
        kept: demo::kept(&demo::chats_state(2, true)),
        menu: None,
        click: None,
        answers: vec![(cmd::ACCOUNT_LIMITS, claude()), (cmd::CODEX_LIMITS, codex())],
        failures: Vec::new(),
        events: Vec::new(),
        act: None,
        window_act: None,
        drive: Vec::new(),
        output: None,
    }
}

/// Five accounts, for the bar giving detail up as it narrows.
fn many() -> Value {
    let names = [("claude", "work"), ("claude", "personal"), ("codex", "main"), ("codex", "spare"), ("claude", "client")];
    Value::Array(names.iter().map(|(p, n)| json!({ "provider": p, "name": n, "path": format!("C:\\accounts\\{p}\\{n}") })).collect())
}

pub fn scenes(theme: ThemePref) -> Vec<Scene> {
    let two_working = || power(true, None, None, 0, &[("c-codex", "busy", true), ("c-site", "busy", true)]);

    let mut bar = base("status-bar", theme);
    bar.events = vec![(event::POWER_STATE, two_working())];

    let mut chips = base("status-chips", theme);
    chips.answers.push((desktop::cmd::UPDATE_CHECK, offer(&demo::core().fingerprint)));
    chips.events = vec![
        (event::CLI_STATUS, json!({ "provider": "claude", "phase": "downloading", "version": null, "got": 31, "total": 100 })),
        (event::CLI_STATUS, json!({ "provider": "codex", "phase": "error", "version": "0.1.0", "error": "offline" })),
        (event::POWER_STATE, two_working()),
    ];

    // Every chip at once, with a countdown and its Cancel: the most the bar
    // has to fit.
    let every_chip = |name| {
        let mut all = base(name, theme);
        all.answers.push((desktop::cmd::UPDATE_CHECK, offer(&demo::core().fingerprint)));
        all.events = vec![
            (event::CLI_STATUS, json!({ "provider": "claude", "phase": "downloading", "version": null, "got": 31, "total": 100 })),
            (event::CLI_STATUS, json!({ "provider": "codex", "phase": "error", "version": "0.1.0", "error": "offline" })),
            (event::POWER_STATE, power(false, Some(("shutdown", 120, false)), Some(NOW_MS + 42_000), 0, &[])),
        ];
        all
    };
    let all = every_chip("status-chips-all");
    // The same on a narrow window, where the update's and the CLIs' chips are
    // under the "..." button: its list opened.
    let mut more = every_chip("status-more");
    more.window_act = Some(|w, _, cx| w.status_bar.update(cx, |s, cx| s.open_more(cx)));

    let mut update = base("status-update-panel", theme);
    update.answers.push((desktop::cmd::UPDATE_CHECK, offer("another-core")));
    update.events = vec![(event::POWER_STATE, power(false, None, None, 0, &[("c-codex", "busy", true), ("c-parser", "waiting", false)]))];
    update.act = Some(|v, cx| v.open_popover(Popover::Update, cx));

    let mut unsaved = base("status-update-unsaved", theme);
    unsaved.answers.push((desktop::cmd::UPDATE_CHECK, offer("another-core")));
    unsaved.failures = vec![(
        desktop::cmd::UPDATE_INSTALL,
        "unsaved: the chat list could not be written (the file is locked by another process, os error 32)",
    )];
    unsaved.act = Some(|v, cx| v.install(cx));

    let mut counting = base("status-countdown", theme);
    counting.events = vec![(event::POWER_STATE, power(false, Some(("shutdown", 120, false)), Some(NOW_MS + 42_000), 0, &[]))];

    let mut panel = base("status-power-panel", theme);
    panel.events = vec![(
        event::POWER_STATE,
        power(
            true,
            Some(("sleep", 300, true)),
            None,
            1,
            &[("c-codex", "busy", true), ("c-site", "busy", true), ("c-parser", "waiting", false)],
        ),
    )];
    panel.act = Some(|v, cx| v.open_popover(Popover::Power, cx));

    let mut crowded = base("status-accounts-many", theme);
    crowded.answers.push((host_cmd::LIST_ACCOUNTS, many()));

    let mut orphan = base("orphan-line", theme);
    orphan.answers.push((cmd::ORPHAN_SESSIONS, orphans()));
    let mut asking = base("orphan-ask", theme);
    asking.answers.push((cmd::ORPHAN_SESSIONS, orphans()));
    asking.act = Some(|v, cx| v.kill_all(cx));

    vec![bar, chips, all, more, update, unsaved, counting, panel, crowded, orphan, asking]
}
