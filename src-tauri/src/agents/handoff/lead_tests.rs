//! A lead's handoff as Luna carries it out: who is listed the tool, when it is
//! asked, the call, the switch at its turn's end, what is given up and why,
//! what a core started again does with it, and what its parent hears. The
//! switch's own state is tested in background/switch.rs, its command lines in
//! background/command.rs.

use super::super::{dir_of, lead_settings_for_test, may_list, sha256_of, THRESHOLD_MAX, THRESHOLD_MIN};
use super::*;
use crate::agents::registry::register;
use crate::settings::LeadHandoff;

const PARENT: &str = "ld-parent";

/// A lead of `provider` under `PARENT`, on conversation "old", with a
/// transcript at `pct` of its context where `pct` is given (Claude Code's
/// shape, as both CLIs' are read alike by `Context::of`), in a folder of the
/// test's own; no copies on disk from a run before.
fn lead(id: &str, pct: Option<u64>) -> std::path::PathBuf {
    if let Ok(dir) = dir_of(id) {
        let _ = std::fs::remove_dir_all(dir);
    }
    let dir = std::env::temp_dir().join(format!("luna-lead-{id}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let at = dir.to_string_lossy().into_owned();
    register(PARENT, Provider::Claude, "C:/p", "C:/acc/anthropic/one", None, true, json!({}));
    register(id, Provider::Claude, &at, &at, Some(PARENT), true, json!({ "model": "sonnet" }));
    crate::background::test_lead(id, Provider::Claude, &at, Some("old"), None);
    if let Some(pct) = pct {
        transcript_at(&at, "old", pct);
    }
    dir
}

/// Conversation `session`'s transcript under `dir`, its last answer at `pct`
/// of a 200k window.
fn transcript_at(dir: &str, session: &str, pct: u64) {
    let path = crate::claude::session::transcript_file(dir, dir, session);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let prompt = r#"{"type":"user","message":{"role":"user","content":"go"}}"#;
    let answer =
        json!({ "type": "assistant", "message": { "model": "claude-sonnet-4-5", "usage": { "input_tokens": pct * 2000 }, "content": [] } });
    std::fs::write(&path, format!("{prompt}\n{answer}\n")).unwrap();
}

fn on(threshold: u8) {
    lead_settings_for_test(Some(LeadHandoff { on: true, threshold }));
}

fn gone(id: &str) {
    crate::background::remove_for_test(id);
    let _ = store::remove(id);
    lead_settings_for_test(None);
}

fn asked(id: &str) -> Option<store::Asked> {
    store::records().get(id).and_then(|r| r.asked.clone())
}

/// A copy of a handoff as the call makes one, and the switch it schedules.
fn scheduled(id: &str, n: u32) -> LeadSwitch {
    let dir = copies_of(id).unwrap();
    let path = dir.join(format!("{n}.md"));
    let text = "# Objective\nFinish the campaign.\n";
    crate::secret::write_private(&path, text.as_bytes()).unwrap();
    let s = LeadSwitch {
        n,
        copy: path.to_string_lossy().into_owned(),
        sha256: sha256_of(text),
        source: "C:\\w\\handoff.md".into(),
        phase: LeadPhase::Scheduled,
        before: Some("old".into()),
        previous: None,
        opening_id: None,
        opened: None,
        from_records: false,
    };
    crate::background::set_switch_for_test(id, Some(s.clone()));
    s
}

/// While the setting is on a lead is listed the tool, and only a lead: a
/// helper without Luna's tools never is, nor hears of it; off, nobody is.
#[test]
fn a_lead_is_listed_the_tool_while_the_setting_is_on_and_a_tool_less_helper_never() {
    lead("ld-list", None);
    let names = |id: &str| -> Vec<String> {
        let r = crate::mcp::handle(id, r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#).unwrap();
        r["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect()
    };
    on(60);
    assert!(may_list("ld-list"));
    assert!(names("ld-list").iter().any(|n| n == "luna_session_handoff"));
    lead_settings_for_test(Some(LeadHandoff { on: false, threshold: 60 }));
    assert!(!may_list("ld-list"));
    assert!(!names("ld-list").iter().any(|n| n == "luna_session_handoff"));
    // Called all the same: a tool there is not.
    let body = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "luna_session_handoff", "arguments": {} } });
    let r = crate::mcp::handle("ld-list", &body.to_string()).unwrap();
    assert_eq!(r["result"]["isError"], true);
    assert!(!r["result"]["content"][0]["text"].as_str().unwrap().contains("ask"), "{r}");

    // A helper without Luna's tools, the setting on.
    on(60);
    register("ld-plain", Provider::Claude, "C:/p", "C:/acc", Some(PARENT), false, json!({}));
    crate::background::test_helper("ld-plain", "C:/acc", Some("s"), false);
    assert!(!may_list("ld-plain"));
    assert!(!names("ld-plain").iter().any(|n| n == "luna_session_handoff"));
    crate::background::remove_for_test("ld-plain");
    gone("ld-list");
}

/// The setting: on at 60% unless set, its threshold held to a chat's range.
#[test]
fn the_setting_is_on_at_sixty_unless_set_and_held_to_the_range() {
    assert_eq!(LeadHandoff::default(), LeadHandoff { on: true, threshold: 60 });
    let s: crate::settings::Settings = serde_json::from_value(json!({})).unwrap();
    assert_eq!(s.lead_handoff, LeadHandoff { on: true, threshold: 60 });
    let s: crate::settings::Settings = serde_json::from_value(json!({ "leadHandoff": { "on": false } })).unwrap();
    assert_eq!(s.lead_handoff, LeadHandoff { on: false, threshold: 60 });
    lead_settings_for_test(Some(LeadHandoff { on: true, threshold: 0 }));
    assert_eq!(lead_settings().threshold, THRESHOLD_MIN);
    lead_settings_for_test(Some(LeadHandoff { on: true, threshold: 200 }));
    assert_eq!(lead_settings().threshold, THRESHOLD_MAX);
    lead_settings_for_test(None);
}

/// Past its threshold at a turn's end, at rest, a lead is asked once, as a
/// turn of Luna's own, the ask bound to the conversation whose context passed
/// it; past the firm mark once more, ahead of anything queued. Short of it,
/// or not at rest, nothing.
#[test]
fn a_lead_past_its_threshold_at_rest_is_asked_as_a_turn_of_lunas_own() {
    lead("ld-ask", Some(50));
    on(60);
    lead_turn_ended("ld-ask");
    assert!(asked("ld-ask").is_none(), "short of the threshold");
    let dir = std::env::temp_dir().join(format!("luna-lead-ld-ask-{}", std::process::id())).to_string_lossy().into_owned();
    transcript_at(&dir, "old", 70);
    // Not at rest: a turn on.
    crate::background::set_running_for_test("ld-ask");
    lead_turn_ended("ld-ask");
    assert!(asked("ld-ask").is_none(), "a turn runs");
    crate::background::lead_turn_over_for_test("ld-ask", &[], None);
    lead_turn_ended("ld-ask");
    let a = asked("ld-ask").expect("asked");
    assert_eq!((a.kind, a.conversation.as_deref()), (store::AskKind::Auto, Some("old")));
    let (_, running, batch, _, _) = crate::background::lead_for_test("ld-ask");
    assert!(running, "the ask is a turn of its own");
    assert!(batch[0].starts_with(crate::agents::NOTICE_TAG) && batch[0].contains("70%") && batch[0].contains("luna_session_handoff"));
    // Once per level.
    crate::background::lead_turn_over_for_test("ld-ask", &[], None);
    lead_turn_ended("ld-ask");
    assert!(!crate::background::lead_for_test("ld-ask").1, "not asked twice at one level");
    // The firm one goes ahead of what waits.
    transcript_at(&dir, "old", 90);
    crate::background::lead_turn_over_for_test("ld-ask", &[], None);
    lead_turn_ended("ld-ask");
    assert!(crate::background::lead_for_test("ld-ask").2[0].contains("90%"));
    crate::background::set_running_for_test("ld-ask");
    crate::background::send_luna("ld-ask", "a notice", false).unwrap();
    crate::background::send_luna("ld-ask", "firm", true).unwrap();
    assert_eq!(crate::background::lead_for_test("ld-ask").3, ["firm", "a notice"]);
    gone("ld-ask");
}

/// Unasked, or asked in a conversation it has left, a lead's call does
/// nothing; asked, without a file it says how, with one it schedules the
/// switch for its turn's end, refused while what its turns left still runs
/// and while one is under way.
#[test]
fn a_leads_call_answers_only_its_own_ask_and_schedules_the_switch() {
    let dir = lead("ld-call", None);
    on(60);
    let file = dir.join("handoff.md");
    std::fs::write(&file, "# Objective\nFinish the campaign.\n").unwrap();
    let file = file.to_string_lossy().into_owned();
    assert_eq!(handoff("ld-call", &file).unwrap_err(), NOT_ASKED);
    assert_eq!(instructions("ld-call").unwrap_err(), NOT_ASKED);
    let ask = |conversation: &str| {
        let a = store::Asked { conversation: Some(conversation.into()), ..store::Asked::new(store::AskKind::Auto, now_ms(), None) };
        store::change("ld-call", |r| r.asked = Some(a)).unwrap();
    };
    ask("another");
    assert_eq!(handoff("ld-call", &file).unwrap_err(), NOT_ASKED, "another conversation was asked");
    ask("old");
    let how = instructions("ld-call").unwrap();
    assert!(how.starts_with("How to hand this session off") && how.contains("As it ends Luna starts"), "{how}");
    // What its turns left still runs: this process stands for it.
    crate::background::lead_turn_over_for_test("ld-call", &[std::process::id()], None);
    let refused = handoff("ld-call", &file).unwrap_err();
    assert!(refused.contains("still running (node"), "{refused}");
    crate::background::lead_turn_over_for_test("ld-call", &[], None);
    let answer = handoff("ld-call", &file).unwrap();
    assert_eq!(answer["scheduled"], true);
    let s = crate::background::switch_of("ld-call").expect("scheduled");
    assert_eq!((s.phase, s.before.as_deref(), s.n), (LeadPhase::Scheduled, Some("old"), answer["n"].as_u64().unwrap() as u32));
    assert_eq!(std::fs::read_to_string(&s.copy).unwrap(), "# Objective\nFinish the campaign.\n");
    assert!(handoff("ld-call", &file).unwrap_err().contains("under way already"));
    assert!(instructions("ld-call").unwrap_err().contains("under way already"));
    let _ = std::fs::remove_dir_all(&dir);
    gone("ld-call");
}

/// At the end of the turn that called, the switch: the conversation let go
/// of, Luna's opening the next turn on none, with the copy, the skills rule,
/// the approvals' bound and the facts; then the conversation its CLI names is
/// the new one, recorded as through, the ask closed, the floor to come, and
/// its parent told with its next notice.
#[test]
fn the_switch_starts_a_fresh_conversation_at_the_turns_end_and_its_parent_hears() {
    lead("ld-switch", Some(70));
    on(60);
    let a = store::Asked { conversation: Some("old".into()), ..store::Asked::new(store::AskKind::Auto, now_ms(), None) };
    store::change("ld-switch", |r| r.asked = Some(a)).unwrap();
    let s = scheduled("ld-switch", 3);
    crate::background::lead_turn_over_for_test("ld-switch", &[], None);
    // Between the turn that handed off and its fresh start it is at work:
    // whoever waits on it waits for what the fresh conversation says.
    let busy = crate::background::turn_state("ld-switch").unwrap();
    assert!(busy.busy && busy.turn == crate::activity::Turn::Busy);
    lead_turn_ended("ld-switch");
    let (session, running, batch, _, _) = crate::background::lead_for_test("ld-switch");
    assert_eq!(session, None, "no conversation it had");
    assert!(running);
    let opening = &batch[0];
    assert!(opening.starts_with("[Luna handoff 3, not typed by the user]"), "{opening}");
    for want in [s.copy.as_str(), "Skills to load first", "no further", "a lead", "What Luna knows now"] {
        assert!(opening.contains(want), "{want}: {opening}");
    }
    let now = crate::background::switch_of("ld-switch").unwrap();
    assert_eq!(now.phase, LeadPhase::Fresh);
    // Its CLI names the new conversation; the turn ends.
    crate::background::name_for_test("ld-switch", "new");
    let dir = std::env::temp_dir().join(format!("luna-lead-ld-switch-{}", std::process::id())).to_string_lossy().into_owned();
    transcript_at(&dir, "new", 70);
    crate::background::lead_turn_over_for_test("ld-switch", &[], None);
    lead_turn_ended("ld-switch");
    assert!(crate::background::switch_of("ld-switch").is_none(), "through");
    let rec = store::records().get("ld-switch").cloned().unwrap();
    assert!(rec.asked.is_none() && rec.past.last().is_some_and(|p| p.n == 3 && p.done));
    // Its first look gave the new conversation its floor: not asked at once,
    // however full it begins.
    let told = rec.told.unwrap();
    assert_eq!((told.conversation.as_str(), told.level, told.floor), ("new", 0, 70));
    let line = reg().lock().unwrap().handed_off.get(PARENT).map(|h| h.line());
    assert_eq!(line.as_deref(), Some("Lead ld-switch handed off (3): it goes on in a fresh conversation; its helpers stay with it."));
    reg().lock().unwrap().handed_off.remove(PARENT);
    gone("ld-switch");
}

/// The switch waits on nothing of the CLI's own: with what its turns left
/// still running, a turn the parent stopped, or a copy that is no longer
/// Luna's, it is given up before its start, the conversation as it was, the
/// ask still open for a call once that is over, and the lead told.
#[test]
fn a_switch_with_work_left_running_or_a_stopped_turn_is_given_up_before_it_starts() {
    for (case, left, error) in
        [("left", vec![std::process::id()], None), ("stopped", vec![], Some(crate::background::STOPPED_BY_STOP)), ("forged", vec![], None)]
    {
        let id = format!("ld-up-{case}");
        lead(&id, Some(70));
        on(60);
        let a = store::Asked { conversation: Some("old".into()), ..store::Asked::new(store::AskKind::Auto, now_ms(), None) };
        store::change(&id, |r| r.asked = Some(a)).unwrap();
        let s = scheduled(&id, 1);
        if case == "forged" {
            std::fs::write(&s.copy, "something else").unwrap();
        }
        crate::background::lead_turn_over_for_test(&id, &left, error);
        lead_turn_ended(&id);
        assert!(crate::background::switch_of(&id).is_none(), "{case}");
        let (session, _, batch, queue, _) = crate::background::lead_for_test(&id);
        assert_eq!(session.as_deref(), Some("old"), "{case}");
        let told = [batch, queue].concat().join("\n");
        assert!(
            told.contains("Handoff 1 did not go through") && told.contains("call luna_session_handoff with file again"),
            "{case}: {told}"
        );
        assert!(asked(&id).is_some(), "{case}: the ask stays open");
        assert!(store::records()[&id].past.last().is_some_and(|p| !p.done), "{case}");
        gone(&id);
    }
}

/// A fresh start that names no conversation (its CLI could not start, or
/// ended first) goes back to the conversation it left, the opening nobody's
/// to be told of, the ask closed so it is not asked over and over.
#[test]
fn a_fresh_start_that_names_no_conversation_goes_back_to_the_one_it_left() {
    // The CLI's own words for why, Codex's here: either CLI's end is the same to it.
    lead("ld-back", Some(70));
    on(60);
    let a = store::Asked { conversation: Some("old".into()), ..store::Asked::new(store::AskKind::Auto, now_ms(), None) };
    store::change("ld-back", |r| r.asked = Some(a)).unwrap();
    scheduled("ld-back", 2);
    crate::background::lead_turn_over_for_test("ld-back", &[], None);
    lead_turn_ended("ld-back");
    assert_eq!(crate::background::switch_of("ld-back").unwrap().phase, LeadPhase::Fresh);
    crate::background::lead_turn_over_for_test("ld-back", &[], Some("codex exited with code 1"));
    lead_turn_ended("ld-back");
    assert!(crate::background::switch_of("ld-back").is_none());
    let (session, _, batch, queue, undelivered) = crate::background::lead_for_test("ld-back");
    assert_eq!(session.as_deref(), Some("old"));
    assert!(undelivered.is_empty());
    let told = [batch, queue].concat().join("\n");
    assert!(told.contains("Handoff 2 started no new conversation: codex exited with code 1"), "{told}");
    assert!(asked("ld-back").is_none());
    gone("ld-back");
}

/// A core started again takes a lead's switch up whatever `resumeOnRestart`
/// says: scheduled, it starts; started with no conversation named, its old
/// opening is dropped from what would go back as undelivered and a new one
/// starts; through, it is recorded. Where nothing says the last core's turn
/// ended, it is given up rather than run beside it.
#[test]
fn a_core_started_again_goes_on_with_a_leads_switch() {
    lead("ld-core", Some(70));
    on(60);
    scheduled("ld-core", 4);
    crate::background::set_running_for_test("ld-core");
    crate::background::lead_across_a_core_for_test("ld-core", false);
    assert!(!crate::background::never_started("ld-core"));
    lead_taken_up("ld-core");
    let (session, running, batch, _, _) = crate::background::lead_for_test("ld-core");
    assert_eq!((session, running), (None, true));
    assert!(batch[0].starts_with("[Luna handoff 4,"));
    // The core goes again with that opening on its way.
    crate::background::lead_across_a_core_for_test("ld-core", false);
    assert!(!crate::background::never_started("ld-core"), "between conversations, not never started");
    assert!(crate::background::lead_for_test("ld-core").4.iter().any(|t| t.starts_with("[Luna handoff 4,")));
    lead_taken_up("ld-core");
    let (_, running, batch, _, undelivered) = crate::background::lead_for_test("ld-core");
    assert!(running && batch.len() == 1 && batch[0].starts_with("[Luna handoff 4,"));
    assert!(!undelivered.iter().any(|t| t.starts_with("[Luna handoff")), "the old opening is nobody's: {undelivered:?}");
    gone("ld-core");

    lead("ld-unsure", Some(70));
    on(60);
    scheduled("ld-unsure", 1);
    crate::background::set_running_for_test("ld-unsure");
    crate::background::lead_across_a_core_for_test("ld-unsure", true);
    lead_taken_up("ld-unsure");
    assert!(crate::background::switch_of("ld-unsure").is_none());
    assert_eq!(crate::background::lead_for_test("ld-unsure").0.as_deref(), Some("old"));
    gone("ld-unsure");
}

/// Mid-switch the person cannot open the lead as a chat: the switch would
/// start a conversation the chat does not have.
#[test]
fn a_lead_is_not_opened_as_a_chat_mid_switch() {
    lead("ld-promote", None);
    scheduled("ld-promote", 1);
    let refused = crate::background::background_promote("ld-promote".into()).err().unwrap();
    assert!(refused.contains("handing off"), "{refused}");
    gone("ld-promote");
}

/// A lead that writes no file is never asked: past its threshold Luna writes
/// its handoff from its records, private in its folder of copies, and its
/// fresh conversation starts on it at once.
#[test]
fn a_lead_that_writes_no_file_goes_on_from_lunas_document() {
    lead("ld-ro", Some(70));
    on(60);
    crate::background::read_only_for_test("ld-ro");
    lead_turn_ended("ld-ro");
    assert!(asked("ld-ro").is_none(), "not asked");
    let s = crate::background::switch_of("ld-ro").expect("switched from records");
    assert!(s.from_records && s.phase == LeadPhase::Fresh);
    let doc = String::from_utf8(crate::secret::read_private(Path::new(&s.copy), file::MAX_BYTES).unwrap().unwrap()).unwrap();
    assert!(doc.starts_with("# Handoff of lead") && doc.contains("Ask your caller") && !doc.contains("person's instruction"), "{doc}");
    let batch = crate::background::lead_for_test("ld-ro").2;
    assert!(batch[0].contains("Luna wrote a handoff of the one before it from its records"), "{}", batch[0]);
    gone("ld-ro");
}

/// The parent's line rides: kept per parent, one lead's again in its place,
/// and gone once a notice told it.
#[test]
fn a_parent_hears_of_its_leads_handoffs_in_one_line() {
    register("ld-rider", Provider::Claude, "C:/p", "C:/acc", Some("ld-rider-top"), true, json!({}));
    register("ld-rider-2", Provider::Claude, "C:/p", "C:/acc", Some("ld-rider-top"), true, json!({}));
    handed_off("ld-rider", 1);
    handed_off("ld-rider", 2);
    handed_off("ld-rider-2", 1);
    let line = reg().lock().unwrap().handed_off["ld-rider-top"].line();
    assert_eq!(line, "Leads handed off: ld-rider (2), ld-rider-2 (1): each goes on in a fresh conversation; their helpers stay with them.");
    reg().lock().unwrap().handed_off.remove("ld-rider-top");
}

/// Luna's own default for a lead tells it to follow the tool once asked.
#[test]
fn the_leads_role_text_names_the_tool() {
    assert!(include_str!("../../../resources/system/role-lead.md").contains("luna_session_handoff"));
    assert!(!include_str!("../../../resources/system/role-helper.md").contains("handoff"));
}
