//! The handoff as the rest of Luna meets it: who is listed the tool, when a
//! call is answered, whose chat a call can touch, what waits while one is
//! under way, and what a chat's deletion takes with it. The machine itself is tested in phase.rs,
//! the file in file.rs, the words in facts.rs, the save in store.rs.

use super::*;
use crate::agents::registry::register;
use crate::agents::teardown::forget as forget_session;
use serde_json::json;

const ACC: &str = "C:/acc/anthropic/one";

/// A chat of the person's with Luna's tools, auto-handoff at `threshold`, and
/// no copies on disk from a run before: tests share the data dir, and one that
/// stopped half way leaves its folder. With a threshold, it has been asked by
/// it, as its notice would (`ask`).
fn chat(id: &str, threshold: Option<u8>) {
    if let Ok(dir) = dir_of(id) {
        let _ = std::fs::remove_dir_all(dir);
    }
    let settings = match threshold {
        Some(t) => json!({ "model": "opus", "handoff": t }),
        None => json!({ "model": "opus" }),
    };
    register(id, Provider::Claude, "C:/p", ACC, None, true, settings);
    if threshold.is_some() {
        ask(id, store::AskKind::Auto);
    }
}

/// `id` asked for a handoff, by its threshold's notice or the person.
fn ask(id: &str, kind: store::AskKind) {
    store::change(id, |r| r.asked = Some(store::Asked { kind, at: now_ms(), turns: None, queued: None, typed: None })).unwrap();
}

fn listed_names(id: &str) -> Vec<String> {
    let r = crate::mcp::handle(id, r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#).unwrap();
    r["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect()
}

fn call(id: &str, args: Value) -> (bool, String) {
    let body =
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "luna_session_handoff", "arguments": args } });
    let r = crate::mcp::handle(id, &body.to_string()).unwrap();
    (r["result"]["isError"] == true, r["result"]["content"][0]["text"].as_str().unwrap_or("").to_string())
}

/// A handoff file in a scratch folder of the test's own.
fn scratch_file(name: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("luna-handoff-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("handoff.md");
    std::fs::write(&file, "# Objective\nFinish the campaign.\n").unwrap();
    (dir, file)
}

fn before() -> Before {
    Before { conversation: Some("old".into()), transcript: Some("/t/old.jsonl".into()) }
}

fn done_with(ids: &[&str], scratch: &Path) {
    for id in ids {
        forget_session(id);
    }
    let _ = std::fs::remove_dir_all(scratch);
}

#[test]
fn the_tool_is_listed_to_every_persons_chat_with_luna_tools() {
    chat("hohv-on", Some(70));
    chat("hohv-off", None);
    register("hohv-kid", Provider::Claude, "C:/p", ACC, Some("hohv-on"), true, json!({ "handoff": 70 }));
    register("hohv-plain", Provider::Claude, "C:/p", ACC, None, false, json!({ "handoff": 70 }));
    chat("hohv-odd", Some(99));
    for id in ["hohv-on", "hohv-off", "hohv-odd"] {
        assert!(listed_names(id).contains(&"luna_session_handoff".to_string()), "{id}: the tick is only the threshold");
        assert!(is_listed(id), "{id}: its CLI has it now");
    }
    assert!(auto_on("hohv-on") && !auto_on("hohv-off") && !auto_on("hohv-odd"), "no threshold Luna takes");
    let names = listed_names("hohv-kid");
    assert!(names.contains(&"luna_session_list".to_string()) && !names.contains(&"luna_session_handoff".to_string()));
    assert!(listed_names("hohv-plain").is_empty(), "no tools: nothing, not even this one");
    // Started again, its CLI lists them anew.
    chat("hohv-on", Some(70));
    assert!(!is_listed("hohv-on"));
    for id in ["hohv-on", "hohv-off", "hohv-kid", "hohv-plain", "hohv-odd"] {
        forget_session(id);
    }
}

/// Listed is not answered: a call, with a file or without, goes through only
/// while someone asked the chat for a handoff, and unasked it is told where
/// the person asks for one.
#[test]
fn a_call_nobody_asked_for_is_refused_naming_the_button() {
    chat("honask-me", None);
    let (dir, file) = scratch_file("nask");
    for args in [json!({}), json!({ "file": file.to_string_lossy() })] {
        let (failed, e) = call("honask-me", args.clone());
        assert!(failed && e.contains("nobody asked this chat for a handoff") && e.contains("Hand off now"), "{args}: {e}");
    }
    assert!(!holds_text("honask-me") && store::records().get("honask-me").is_none_or(|r| r.served.is_none()), "nothing kept");
    // Asked by the person: answered.
    ask("honask-me", store::AskKind::Manual);
    let (failed, how) = call("honask-me", json!({}));
    assert!(!failed && how.contains("How to hand this chat off"), "{how}");
    done_with(&["honask-me"], &dir);
}

#[test]
fn a_call_reaches_only_the_callers_own_chat() {
    chat("hont-me", Some(70));
    chat("hont-other", Some(70));
    chat("hont-off", None);
    register("hont-kid", Provider::Claude, "C:/p", ACC, Some("hont-me"), true, json!({ "handoff": 70 }));
    // No argument names a chat: one that tries is refused before anything runs.
    for key in ["id", "target", "chat", "scope"] {
        let (failed, e) = call("hont-me", json!({ "file": "C:/x/h.md", key: "hont-other" }));
        assert!(failed && e.contains(&format!("no argument `{key}`")) && e.contains("takes: file"), "{key}: {e}");
    }
    // Without a file it only answers how: nothing is scheduled.
    let (failed, how) = call("hont-me", json!({}));
    assert!(!failed && how.contains("How to hand this chat off") && !holds_text("hont-me"), "{how}");
    // Not listed: refused as a tool there is not, whoever's file it names.
    let (failed, e) = call("hont-kid", json!({ "file": "C:/x/h.md" }));
    assert!(failed && e.contains("unknown tool luna_session_handoff"), "{e}");
    // Listed, not asked: refused, nothing run.
    let (failed, e) = call("hont-off", json!({ "file": "C:/x/h.md" }));
    assert!(failed && e.contains("nobody asked"), "{e}");
    let (failed, e) = call(DEV_CALLER, json!({ "file": "C:/x/h.md" }));
    assert!(failed, "{e}");
    // Offered, but not running in a terminal here: nothing scheduled.
    let (failed, e) = call("hont-me", json!({ "file": "C:/x/h.md" }));
    assert!(failed && e.contains("not running"), "{e}");
    // What one chat schedules is its own: no other's record moves.
    let (dir, file) = scratch_file("nt");
    schedule("hont-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    assert!(holds_text("hont-me") && !holds_text("hont-other"));
    assert!(store::records().get("hont-other").is_some_and(|r| r.switch.is_none() && r.last == 0));
    done_with(&["hont-me", "hont-other", "hont-off", "hont-kid"], &dir);
}

#[test]
fn a_call_is_refused_while_the_clis_own_jobs_run_naming_them() {
    chat("hooj-me", Some(70));
    crate::activity::subagents_for_test("hooj-me", 2);
    let (failed, e) = call("hooj-me", json!({ "file": "C:/x/h.md" }));
    assert!(failed && e.contains("2 subagents") && e.contains("cannot cross"), "{e}");
    assert!(!holds_text("hooj-me"), "nothing scheduled");
    crate::activity::subagents_for_test("hooj-me", 0);
    crate::activity::forget("hooj-me");
    forget_session("hooj-me");
}

#[test]
fn a_schedule_copies_numbers_and_refuses_a_second_under_way() {
    chat("hosc-me", Some(70));
    let (dir, file) = scratch_file("sc");
    // A threshold notice not yet typed goes: the new conversation has nothing to hand off.
    reg().lock().unwrap().news.push(("hosc-me".into(), facts::notice(75, 70, false, true)));
    reg().lock().unwrap().news.push(("hosc-me".into(), "Your helper h1 answered.".into()));
    let v = schedule("hosc-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    assert_eq!((v["scheduled"].clone(), v["n"].clone()), (json!(true), json!(1)));
    let snapshot = PathBuf::from(v["snapshot"].as_str().unwrap());
    assert_eq!(snapshot, dir_of("hosc-me").unwrap().join("1.md"));
    assert_eq!(std::fs::read_to_string(&snapshot).unwrap(), "# Objective\nFinish the campaign.\n");
    let news: Vec<String> = reg().lock().unwrap().news.iter().filter(|(c, _)| c == "hosc-me").map(|(_, l)| l.clone()).collect();
    assert_eq!(news, ["Your helper h1 answered."]);
    let e = schedule("hosc-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap_err();
    assert!(e.contains("handoff 1 is under way"), "{e}");
    assert_eq!(mark_of("hosc-me").map(|m| (m.n, m.state)), Some((1, "underWay")));
    // Through: on record, and the next is number 2.
    finish("hosc-me", 1, Ended::Done, &String::new);
    assert!(!holds_text("hosc-me"));
    assert_eq!(mark_of("hosc-me").map(|m| (m.n, m.state)), Some((1, "done")));
    assert_eq!(handoff_file("hosc-me".into(), 1).unwrap(), "# Objective\nFinish the campaign.\n");
    assert_eq!(open_ask("hosc-me"), None, "the ask it answered closed with it");
    ask("hosc-me", store::AskKind::Auto);
    let v = schedule("hosc-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    assert_eq!(v["n"], 2);
    reg().lock().unwrap().news.retain(|(c, _)| c != "hosc-me");
    done_with(&["hosc-me"], &dir);
}

#[test]
fn notices_and_sends_wait_while_a_handoff_is_under_way() {
    chat("hohw-me", Some(70));
    chat("hohw-sender", None);
    let (dir, file) = scratch_file("hw");
    assert!(!crate::agents::notifier::notice_waits("hohw-me"));
    schedule("hohw-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    assert!(crate::agents::notifier::notice_waits("hohw-me"), "the notifier holds what it has for the chat");
    // Text another session sends goes in after, into the new conversation.
    let sent = crate::agents::send("hohw-sender", "hohw-me", "the build is green", false).unwrap();
    assert!(sent.held && !sent.started);
    assert!(crate::agents::held::is_held("hohw-sender", "hohw-me"));
    // A command is still refused as one, held or not.
    assert!(crate::agents::send("hohw-sender", "hohw-me", "/exit", false).is_err());
    finish("hohw-me", 1, Ended::Done, &String::new);
    assert!(!crate::agents::notifier::notice_waits("hohw-me"), "released once it is through");
    reg().lock().unwrap().held.retain(|h| h.id != "hohw-me");
    done_with(&["hohw-me", "hohw-sender"], &dir);
}

#[test]
fn a_handoff_given_up_before_its_reset_tells_the_old_conversation() {
    chat("hogu-me", Some(70));
    let (dir, file) = scratch_file("gu");
    schedule("hogu-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    finish(
        "hogu-me",
        1,
        Ended::Failed { why: "the CLI did not start a new conversation within 60 s of the reset".into(), reset: false },
        &|| "PROMPT".into(),
    );
    assert!(!holds_text("hogu-me"), "released");
    let news: Vec<String> = reg().lock().unwrap().news.iter().filter(|(c, _)| c == "hogu-me").map(|(_, l)| l.clone()).collect();
    assert_eq!(news.len(), 1);
    assert!(news[0].contains("did not go through") && news[0].contains("goes on as it was") && !news[0].contains("PROMPT"), "{}", news[0]);
    assert!(news[0].contains("Call luna_session_handoff again"), "the tool is still its: {}", news[0]);
    assert_eq!(mark_of("hogu-me").map(|m| (m.state, m.why.is_some())), Some(("failed", true)));
    // After the reset, the new conversation hears where to start.
    reg().lock().unwrap().news.retain(|(c, _)| c != "hogu-me");
    schedule("hogu-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    finish("hogu-me", 2, Ended::Failed { why: "the new conversation did not take Luna's prompt within 60 s".into(), reset: true }, &|| {
        "PROMPT".into()
    });
    let news: Vec<String> = reg().lock().unwrap().news.iter().filter(|(c, _)| c == "hogu-me").map(|(_, l)| l.clone()).collect();
    assert!(news[0].contains("cleared the conversation") && news[0].ends_with("PROMPT"), "{}", news[0]);
    reg().lock().unwrap().news.retain(|(c, _)| c != "hogu-me");
    done_with(&["hogu-me"], &dir);
}

#[test]
fn turning_it_off_closes_the_thresholds_ask_and_gives_up_its_scheduled_handoff() {
    chat("hots-me", None);
    assert!(!auto_on("hots-me") && may_list("hots-me"));
    assert!(set_auto_handoff("hots-me".into(), Some(91)).unwrap_err().contains("not one"));
    assert!(set_auto_handoff("hots-me".into(), Some(0)).is_err());
    // On: the registry has it now; no CLI runs here, so nothing to resume.
    assert_eq!(set_auto_handoff("hots-me".into(), Some(60)), Ok(AutoHandoff { resume: false }));
    assert!(auto_on("hots-me"));
    assert_eq!(crate::agents::settings_of("hots-me")["handoff"], 60);
    assert_eq!(open_ask("hots-me"), None, "on asks nothing by itself: its threshold does");
    ask("hots-me", store::AskKind::Auto);
    let (dir, file) = scratch_file("ts");
    schedule("hots-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    // Off: the threshold's ask closes, and what was only scheduled from it is given up.
    set_auto_handoff("hots-me".into(), None).unwrap();
    assert!(!auto_on("hots-me") && crate::agents::settings_of("hots-me").get("handoff").is_none());
    assert!(may_list("hots-me"), "still listed: the person may still ask");
    assert_eq!(open_ask("hots-me"), None);
    assert!(!holds_text("hots-me"));
    assert_eq!(mark_of("hots-me").and_then(|m| m.why), Some("auto-handoff was turned off for this chat".into()));
    let told: Vec<String> = reg().lock().unwrap().news.iter().filter(|(c, _)| c == "hots-me").map(|(_, l)| l.clone()).collect();
    assert!(told[0].contains("Nobody asks this chat for a handoff any more"), "not asked to call again: {}", told[0]);
    assert!(!told[0].contains("turns it on"), "{}", told[0]);
    let (failed, e) = call("hots-me", json!({}));
    assert!(failed && e.contains("nobody asked"), "{e}");
    reg().lock().unwrap().news.retain(|(c, _)| c != "hots-me");
    done_with(&["hots-me"], &dir);
}

/// The person's Hand off now: an ask of theirs, which turning auto-handoff
/// off leaves be with the handoff answering it, which a core restart keeps,
/// and which lapses MANUAL_ASK_MS after its request went in, the chat and the
/// person told.
#[test]
fn the_persons_ask_survives_auto_handoff_off_and_a_restart_and_lapses() {
    chat("homan-me", Some(70));
    assert_eq!(open_ask("homan-me"), Some(store::AskKind::Auto));
    reg().lock().unwrap().news.push(("homan-me".into(), facts::notice(75, 70, false, true)));
    assert_eq!(open_manual("homan-me"), Ok(HandoffNow { resume: true }), "its CLI never listed the tool here");
    assert_eq!(open_ask("homan-me"), Some(store::AskKind::Manual), "in the threshold's place");
    let news = |id: &str| -> Vec<String> { reg().lock().unwrap().news.iter().filter(|(c, _)| c == id).map(|(_, l)| l.clone()).collect() };
    assert!(news("homan-me").is_empty(), "the threshold's notice not yet typed would ask a second time");
    assert!(now_refusal("homan-me").is_some_and(|e| e.contains("asked for a handoff already")));
    // Off: the person's ask, and the handoff answering it, stay.
    let (dir, file) = scratch_file("man");
    schedule("homan-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    set_auto_handoff("homan-me".into(), None).unwrap();
    assert_eq!(open_ask("homan-me"), Some(store::AskKind::Manual));
    assert!(holds_text("homan-me"), "the switch goes on");
    // A core started later takes it up as it was.
    let saved = serde_json::to_vec(&*store::records()).unwrap();
    let back = store::from_bytes(&saved, 5);
    assert_eq!(back["homan-me"].asked, store::records()["homan-me"].asked);
    assert!(back["homan-me"].asked.is_some());
    finish("homan-me", 1, Ended::Done, &String::new);
    assert_eq!(open_ask("homan-me"), None, "answered");
    // Asked again: its request queued and gone in, then its time runs out.
    open_manual("homan-me").unwrap();
    let at = store::records()["homan-me"].asked.as_ref().unwrap().at;
    store::change("homan-me", |r| r.asked.as_mut().unwrap().queued = Some(at)).unwrap();
    reg().lock().unwrap().news.push(("homan-me".into(), facts::manual_notice()));
    manual_ask_of("homan-me", at + 1);
    assert_eq!(store::records()["homan-me"].asked.as_ref().unwrap().typed, None, "still queued");
    reg().lock().unwrap().news.retain(|(c, _)| c != "homan-me");
    manual_ask_of("homan-me", at + 2);
    assert_eq!(store::records()["homan-me"].asked.as_ref().unwrap().typed, Some(at + 2), "gone in");
    manual_ask_of("homan-me", at + 2 + MANUAL_ASK_MS);
    assert_eq!(open_ask("homan-me"), Some(store::AskKind::Manual), "not before its time");
    manual_ask_of("homan-me", at + 3 + MANUAL_ASK_MS);
    assert_eq!(open_ask("homan-me"), None, "lapsed");
    assert_eq!(news("homan-me"), [facts::manual_lapsed(MANUAL_ASK_MS / 60_000)]);
    let (failed, e) = call("homan-me", json!({}));
    assert!(failed && e.contains("nobody asked"), "{e}");
    reg().lock().unwrap().news.retain(|(c, _)| c != "homan-me");
    done_with(&["homan-me"], &dir);
}

/// Hand off now is refused, with what stands in the way, where the chat
/// could not answer it.
#[test]
fn hand_off_now_is_refused_where_the_chat_cannot_answer_saying_why() {
    chat("honow-me", None);
    register("honow-kid", Provider::Claude, "C:/p", ACC, Some("honow-me"), true, json!({}));
    register("honow-plain", Provider::Claude, "C:/p", ACC, None, false, json!({}));
    register("honow-ro", Provider::Codex, "C:/p", "C:/acc/openai/two", None, true, json!({ "sandbox": "read-only" }));
    register("honow-plan", Provider::Claude, "C:/p", ACC, None, true, json!({ "permissionMode": "plan" }));
    let refused = |id: &str| now_refusal(id).unwrap_or_default();
    for id in ["honow-kid", "honow-plain", DEV_CALLER] {
        assert!(refused(id).contains("started with Luna's tools"), "{id}: {}", refused(id));
        assert!(handoff_now(id.into()).is_err());
    }
    assert!(refused("honow-ro").contains("read-only sandbox"), "{}", refused("honow-ro"));
    assert!(refused("honow-plan").contains("plan mode"), "{}", refused("honow-plan"));
    // Its CLI's own jobs cannot cross.
    crate::activity::subagents_for_test("honow-me", 1);
    assert!(refused("honow-me").contains("own jobs running"), "{}", refused("honow-me"));
    crate::activity::subagents_for_test("honow-me", 0);
    crate::activity::forget("honow-me");
    // Nothing in the way here but its CLI, which does not run in a terminal here.
    assert_eq!(now_refusal("honow-me"), None);
    assert!(handoff_now("honow-me".into()).unwrap_err().contains("not running"));
    assert_eq!(open_ask("honow-me"), None, "nothing opened");
    // A move asking the chat for its document.
    let asking = moving::tests::a_move(moving::MovePhase::Asking { at: 1 });
    store::change("honow-me", |r| r.moving = Some(asking)).unwrap();
    assert!(refused("honow-me").contains("move's document"), "{}", refused("honow-me"));
    assert!(open_manual("honow-me").is_err(), "looked at again where it is opened");
    store::change("honow-me", |r| r.moving = None).unwrap();
    // A handoff under way.
    ask("honow-me", store::AskKind::Auto);
    let (dir, file) = scratch_file("now");
    schedule("honow-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    assert!(refused("honow-me").contains("handoff 1 of this chat is under way"), "{}", refused("honow-me"));
    assert!(open_manual("honow-me").is_err());
    finish("honow-me", 1, Ended::Done, &String::new);
    done_with(&["honow-me", "honow-kid", "honow-plain", "honow-ro", "honow-plan"], &dir);
}

#[test]
fn a_chat_deleted_takes_its_handoffs_with_it() {
    chat("hofg-me", Some(70));
    let (dir, file) = scratch_file("fg");
    schedule("hofg-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    let kept = dir_of("hofg-me").unwrap();
    assert!(kept.join("1.md").is_file());
    forget_session("hofg-me");
    assert!(!kept.exists() && store::records().get("hofg-me").is_none() && !holds_text("hofg-me"));
    assert!(dir_of("../x").is_err() && dir_of("a/b").is_err() && dir_of("").is_err(), "only a plain id is made a path");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_facts_name_the_chats_helpers_and_what_they_owe() {
    chat("hogf-me", Some(70));
    register(
        "hogf-kid",
        Provider::Codex,
        "C:/p",
        "C:/acc/openai/two",
        Some("hogf-me"),
        false,
        json!({ "background": true, "model": "gpt-big", "effort": "high" }),
    );
    reg().lock().unwrap().owed.insert(("hogf-me".into(), "hogf-kid".into()), crate::agents::owed::Owed::from(0, 0));
    let f = gather("hogf-me");
    assert_eq!(f.helpers.len(), 1);
    assert_eq!((f.helpers[0].id.as_str(), f.helpers[0].owes, f.helpers[0].account.as_str()), ("hogf-kid", true, "codex/two"));
    assert_eq!((f.helpers[0].model.as_deref(), f.helpers[0].effort.as_deref()), (Some("gpt-big"), Some("high")));
    assert_eq!(f.threshold, Some(70));
    assert!(f.owed_by_others.is_empty());
    forget_session("hogf-kid");
    forget_session("hogf-me");
}

#[test]
fn a_handoff_given_up_between_the_look_and_the_reset_types_nothing() {
    chat("hocr-me", Some(70));
    let (dir, file) = scratch_file("cr");
    schedule("hocr-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    // The run reads the switch, then auto-handoff is turned off before its reset.
    let t = phase::tests::fake(|_, _| {});
    *t.on_look.borrow_mut() = Some(Box::new(|| {
        set_auto_handoff("hocr-me".into(), None).unwrap();
    }));
    assert_eq!(phase::run(&t, "hocr-me", 1, &|| "prompt".into(), &Stored), Ended::Back);
    assert!(t.typed.borrow().is_empty(), "no /clear with no handoff to follow it");
    assert_eq!(mark_of("hocr-me").map(|m| m.state), Some("failed"));
    // A switch that has taken its reset is not given up by turning it off: the
    // prompt is still to follow.
    set_auto_handoff("hocr-me".into(), Some(70)).unwrap();
    ask("hocr-me", store::AskKind::Auto);
    schedule("hocr-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    Stored.advance("hocr-me", 2, Phase::Scheduled, Phase::Resetting { at: 1 }, 1).unwrap();
    set_auto_handoff("hocr-me".into(), None).unwrap();
    assert!(holds_text("hocr-me"), "the switch goes on to its prompt");
    finish("hocr-me", 2, Ended::Done, &String::new);
    reg().lock().unwrap().news.retain(|(c, _)| c != "hocr-me");
    done_with(&["hocr-me"], &dir);
}

#[test]
fn what_cannot_be_saved_is_not_typed_and_the_chat_hears_why() {
    chat("hods-me", Some(70));
    let (dir, file) = scratch_file("ds");
    // The schedule itself: refused, and its copy gone.
    store::failing().lock().unwrap().insert("hods-me".into());
    let e = schedule("hods-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap_err();
    assert!(e.contains("could not be saved"), "{e}");
    assert!(!holds_text("hods-me") && !dir_of("hods-me").unwrap().join("1.md").exists());
    // The reset: its phase will not save, so no /clear goes in.
    store::failing().lock().unwrap().remove("hods-me");
    schedule("hods-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    store::failing().lock().unwrap().insert("hods-me".into());
    let t = phase::tests::fake(|_, _| {});
    let ended = phase::run(&t, "hods-me", 2, &|| "prompt".into(), &Stored);
    assert!(t.typed.borrow().is_empty());
    assert!(matches!(&ended, Ended::Failed { why, reset: false } if why.contains("could not be saved")), "{ended:?}");
    finish("hods-me", 2, ended, &|| "prompt".into());
    assert!(!holds_text("hods-me"), "let go, though the disk would not take it");
    let told: Vec<String> = reg().lock().unwrap().news.iter().filter(|(c, _)| c == "hods-me").map(|(_, l)| l.clone()).collect();
    assert!(told[0].contains("could not be saved"), "{}", told[0]);
    store::failing().lock().unwrap().remove("hods-me");
    reg().lock().unwrap().news.retain(|(c, _)| c != "hods-me");
    done_with(&["hods-me"], &dir);
}

#[test]
fn a_schedule_waits_for_what_is_going_into_the_chat_and_then_nothing_goes() {
    chat("hoex-me", Some(70));
    let (dir, file) = scratch_file("ex");
    // A notice or a held send is on its way into the chat, under its exclusion.
    let going = crate::agents::notifier::going_into("hoex-me").expect("nothing stands in its way yet");
    let path = file.to_string_lossy().into_owned();
    let started = std::time::Instant::now();
    let scheduling = std::thread::spawn(move || schedule("hoex-me", Provider::Claude, &path, before()));
    std::thread::sleep(std::time::Duration::from_millis(400));
    assert!(!holds_text("hoex-me"), "not scheduled while text is going in");
    drop(going);
    scheduling.join().unwrap().unwrap();
    assert!(started.elapsed() >= std::time::Duration::from_millis(400));
    // Scheduled: what would type into it now is refused under the same exclusion.
    assert!(crate::agents::notifier::going_into("hoex-me").is_none());
    assert!(crate::agents::notifier::tell_of("hoex-me", &[]).is_none(), "a notice checks again where it goes in");
    finish("hoex-me", 1, Ended::Done, &String::new);
    assert!(crate::agents::notifier::going_into("hoex-me").is_some());
    done_with(&["hoex-me"], &dir);
}

#[test]
fn the_file_is_read_outside_the_records_lock_one_call_at_a_time() {
    chat("horl-me", Some(70));
    let (dir, file) = scratch_file("rl");
    let path = file.to_string_lossy().into_owned();
    // While the file is being read, every other chat's record is free, and a
    // second call of this chat's is refused rather than racing it.
    let free = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let seen = free.clone();
    file::on_read(&path, move || {
        // Other tests take the records for a moment: one free look of a few is enough.
        let free = (0..20).any(|_| {
            let got = store::try_records().is_some();
            std::thread::sleep(std::time::Duration::from_millis(5));
            got
        });
        seen.store(free, std::sync::atomic::Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(300));
    });
    let p = path.clone();
    let first = std::thread::spawn(move || schedule("horl-me", Provider::Claude, &p, before()));
    std::thread::sleep(std::time::Duration::from_millis(100));
    let e = schedule("horl-me", Provider::Claude, &path, before()).unwrap_err();
    assert!(e.contains("being scheduled already"), "{e}");
    assert_eq!(first.join().unwrap().unwrap()["n"], 1);
    assert!(free.load(std::sync::atomic::Ordering::SeqCst), "the records were not held across the read");
    done_with(&["horl-me"], &dir);
}

#[test]
fn a_copy_a_stopped_core_left_behind_is_never_written_into() {
    chat("holb-me", Some(70));
    let (dir, file) = scratch_file("lb");
    // A core stopped between the copy and its record: the copy is there, the
    // number in no record.
    let kept = dir_of("holb-me").unwrap();
    std::fs::create_dir_all(&kept).unwrap();
    std::fs::write(kept.join("1.md"), "left behind").unwrap();
    let v = schedule("holb-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    assert_eq!(v["n"], 2);
    assert_eq!(std::fs::read_to_string(kept.join("1.md")).unwrap(), "left behind");
    done_with(&["holb-me"], &dir);
}

#[test]
fn turning_it_off_while_the_file_is_read_leaves_nothing_to_switch() {
    chat("hooff-me", Some(70));
    let (dir, file) = scratch_file("hooff");
    let path = file.to_string_lossy().into_owned();
    // The person turns it off as the file is being read.
    file::on_read(&path, || {
        set_auto_handoff("hooff-me".into(), None).unwrap();
    });
    let e = schedule("hooff-me", Provider::Claude, &path, before()).unwrap_err();
    assert!(e.contains("nothing will switch"), "{e}");
    assert!(!holds_text("hooff-me"), "no switch to reset the chat with");
    assert!(!dir_of("hooff-me").unwrap().join("1.md").exists(), "its copy gone");
    // Or the chat starts again without Luna's tools as the file is read: the same.
    set_auto_handoff("hooff-me".into(), Some(70)).unwrap();
    ask("hooff-me", store::AskKind::Auto);
    file::on_read(&path, || {
        register("hooff-me", Provider::Claude, "C:/p", ACC, None, false, json!({ "model": "opus" }));
    });
    let e = schedule("hooff-me", Provider::Claude, &path, before()).unwrap_err();
    assert!(e.contains("closed while the file was copied") && e.contains("nothing will switch"), "{e}");
    assert!(!holds_text("hooff-me"));
    reg().lock().unwrap().news.retain(|(c, _)| c != "hooff-me");
    done_with(&["hooff-me"], &dir);
}

/// Only a person's chat with Luna's tools knows of the handoff: a helper, a
/// chat without Luna's tools and the dev caller are not offered the tool, are
/// told nothing of it at their start, and calling it all the same are refused
/// as for a tool there is not, with nothing kept for them. No session finds a
/// topic of it in the guide: the tool speaks for itself. A chat's threshold
/// is its own to know of.
#[test]
fn nobody_but_a_persons_chat_with_luna_tools_hears_of_it() {
    chat("hoaud-me", Some(70));
    chat("hoaud-tools", None);
    register("hoaud-kid", Provider::Claude, "C:/p", ACC, Some("hoaud-me"), true, json!({ "handoff": 70 }));
    register("hoaud-plain", Provider::Claude, "C:/p", ACC, None, false, json!({ "handoff": 70 }));
    let tells = |text: &str| {
        let t = text.to_lowercase();
        ["auto-handoff", "session-handoff", "what luna applied", "takes: file", "hand this chat off", "hand off now"]
            .iter()
            .any(|w| t.contains(w))
    };
    for id in ["hoaud-kid", "hoaud-plain", DEV_CALLER] {
        assert!(!listed_names(id).iter().any(|n| n.contains("handoff")), "{id}");
        let init = crate::mcp::handle(id, r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#).unwrap();
        assert!(!init.to_string().to_lowercase().contains("handoff"), "{id}: {init}");
        for args in [json!({}), json!({ "file": "C:/x/h.md" }), json!({ "chat": "hoaud-me" })] {
            let (failed, e) = call(id, args.clone());
            assert!(failed && !tells(&e), "{id} {args}: {e}");
        }
        assert!(!holds_text(id) && store::records().get(id).is_none(), "{id}: nothing kept for it");
    }
    for id in ["hoaud-me", "hoaud-tools", "hoaud-kid", DEV_CALLER] {
        let guide = json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": "luna_guide_get", "arguments": { "topic": "luna_session_handoff" } } });
        let r = crate::mcp::handle(id, &guide.to_string()).unwrap();
        assert_eq!(r["result"]["isError"], true, "{id}: no such topic");
    }
    // Its row as another session reads it: a helper of its own asking by id,
    // another chat with `scope: "all"`, a listing: its settings without its
    // auto-handoff. The chat itself reads them whole.
    let tool = |caller: &str, name: &str, args: Value| {
        let body = json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": { "name": name, "arguments": args } });
        let r = crate::mcp::handle(caller, &body.to_string()).unwrap();
        assert!(r["result"].get("isError").is_none(), "{caller} {name}: {r}");
        r["result"]["content"][0]["text"].as_str().unwrap_or("").to_string()
    };
    for (caller, name, args) in [
        ("hoaud-kid", "luna_session_get", json!({ "id": "hoaud-me" })),
        ("hoaud-tools", "luna_session_get", json!({ "id": "hoaud-me", "scope": "all" })),
        ("hoaud-tools", "luna_session_list", json!({ "ids": ["hoaud-me"], "scope": "all" })),
    ] {
        let text = tool(caller, name, args);
        assert!(text.contains("hoaud-me") && !text.contains("handoff"), "{caller} {name}: {text}");
    }
    let to = |caller: &str| crate::agents::list::list(caller, Some(&["hoaud-me".to_string()]), true).sessions[0].settings.clone();
    assert_eq!((to("hoaud-me")["handoff"].clone(), to("hoaud-kid").get("handoff")), (json!(70), None));
    assert_eq!(to("hoaud-kid")["model"], "opus", "the rest of its settings as they are");
    // The person's chats: the tool, and how to write by once asked.
    for id in ["hoaud-me", "hoaud-tools"] {
        assert!(listed_names(id).contains(&"luna_session_handoff".to_string()), "{id}");
    }
    let (failed, how) = call("hoaud-me", json!({}));
    assert!(!failed && how.contains("How to hand this chat off"), "{how}");
    for id in ["hoaud-me", "hoaud-tools", "hoaud-kid", "hoaud-plain"] {
        forget_session(id);
    }
}

/// The folder of handoffs, a chat's, its drafts: each checked, top down,
/// before anything is made below it or given to a CLI. A link planted at any
/// of them (a junction on Windows), or a folder of another user's on the way,
/// is refused, nothing made through it, and no drafts folder is named.
#[test]
fn a_link_or_a_strangers_folder_on_the_way_to_the_drafts_is_refused() {
    let base = std::env::temp_dir().join(format!("luna-handoff-chain-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let names = ["c1", DRAFTS];
    let made = private_chain(&base.join("ok").join("handoffs"), &names).unwrap();
    assert_eq!(made, base.join("ok").join("handoffs").join("c1").join(DRAFTS));
    assert!(made.is_dir());
    #[cfg(windows)]
    assert!(crate::secret::acl::is_private(&made).unwrap() && crate::secret::acl::is_private(made.parent().unwrap()).unwrap());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for d in [&made, &made.parent().unwrap().to_path_buf()] {
            assert_eq!(std::fs::metadata(d).unwrap().permissions().mode() & 0o777, 0o700, "{}", d.display());
        }
    }
    for at in 0..=names.len() {
        let top = base.join(format!("link{at}")).join("handoffs");
        let elsewhere = base.join(format!("elsewhere{at}"));
        std::fs::create_dir_all(&elsewhere).unwrap();
        let planted = names[..at].iter().fold(top.clone(), |p, n| p.join(n));
        std::fs::create_dir_all(planted.parent().unwrap()).unwrap();
        assert!(file::tests::dir_link(&elsewhere, &planted), "a link at {}", planted.display());
        let e = private_chain(&top, &names).unwrap_err();
        assert!(e.contains("no private folder of Luna's"), "{}: {e}", planted.display());
        assert!(std::fs::read_dir(&elsewhere).unwrap().next().is_none(), "nothing made through the link at {}", planted.display());
    }
    // A folder of another user's on the way: where this test may hand one over.
    #[cfg(unix)]
    if unsafe { libc::geteuid() } == 0 {
        let top = base.join("stranger").join("handoffs");
        std::fs::create_dir_all(top.join("c1")).unwrap();
        std::os::unix::fs::chown(top.join("c1"), Some(4242), Some(4242)).unwrap();
        let e = private_chain(&top, &names).unwrap_err();
        assert!(e.contains("another user"), "{e}");
        assert!(!top.join("c1").join(DRAFTS).exists(), "nothing made in it");
    }
    let _ = std::fs::remove_dir_all(&base);
}

/// A switch, a fresh one or one a core took up again, resets and prompts only
/// on the copy Luna made: changed since, a link at its name, or outside the
/// chat's folder of copies, nothing is typed, and the chat hears why without
/// a prompt that would point at it.
#[test]
fn a_copy_that_is_not_the_one_luna_made_is_never_switched_on() {
    chat("hocp-me", Some(70));
    let (dir, file) = scratch_file("cp");
    schedule("hocp-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    let s = store::records()["hocp-me"].switch.clone().unwrap();
    let copy = PathBuf::from(&s.snapshot);
    let made = std::fs::read(&copy).unwrap();
    assert!(copy_holds("hocp-me", &s).is_ok());
    // Not where the record's folder of copies is.
    let elsewhere = Switch { snapshot: file.to_string_lossy().into_owned(), ..s.clone() };
    assert!(copy_holds("hocp-me", &elsewhere).unwrap_err().contains("not in this chat's folder"));
    // A link at its name to a file of the very same bytes, where links can be made here.
    let same = dir.join("same.md");
    std::fs::write(&same, &made).unwrap();
    std::fs::remove_file(&copy).unwrap();
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_file(&same, &copy).is_ok();
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&same, &copy).is_ok();
    if linked {
        assert!(copy_holds("hocp-me", &s).unwrap_err().contains("not read"));
        std::fs::remove_file(&copy).unwrap();
    }
    // Changed since it was copied: its hash says so, and the run types nothing.
    crate::secret::write_private(&copy, b"# Goal\nForged.\n").unwrap();
    assert!(copy_holds("hocp-me", &s).unwrap_err().contains("SHA-256 differs"));
    let t = phase::tests::fake(|_, _| {});
    switch_on(&t, "hocp-me", &s);
    assert!(t.typed.borrow().is_empty() && !holds_text("hocp-me"), "no reset");
    assert!(mark_of("hocp-me").and_then(|m| m.why).is_some_and(|w| w.contains("its copy cannot be trusted")));
    let told: Vec<String> = reg().lock().unwrap().news.iter().filter(|(c, _)| c == "hocp-me").map(|(_, l)| l.clone()).collect();
    assert!(told[0].contains("did not go through") && told[0].contains("cannot be trusted"), "{}", told[0]);
    // Taken up after its reset: the new conversation is told, with no prompt
    // pointing at the copy.
    reg().lock().unwrap().news.retain(|(c, _)| c != "hocp-me");
    schedule("hocp-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    let mut s = store::records()["hocp-me"].switch.clone().unwrap();
    s.phase = Phase::Recovering { since: 1 };
    store::change("hocp-me", |r| r.switch = Some(s.clone())).unwrap();
    crate::secret::write_private(Path::new(&s.snapshot), b"# Goal\nForged.\n").unwrap();
    switch_on(&t, "hocp-me", &s);
    let told: Vec<String> = reg().lock().unwrap().news.iter().filter(|(c, _)| c == "hocp-me").map(|(_, l)| l.clone()).collect();
    assert!(told[0].contains("gives the new one no handoff") && !told[0].contains(PROMPT_TAG), "{}", told[0]);
    reg().lock().unwrap().news.retain(|(c, _)| c != "hocp-me");
    done_with(&["hocp-me"], &dir);
}

/// The drafts folder is made private at every start of the chat's CLI,
/// before it runs, a foreign ACE added while it was open going too; after
/// it, what the CLI's sandbox grants itself there (Codex 0.167.0 on Windows)
/// stays through every later look, the answer naming a draft among them; a
/// link at its name is no drafts folder at any look.
#[cfg(windows)]
#[test]
fn the_drafts_folder_is_made_private_at_the_start_and_keeps_what_its_cli_adds_after() {
    use crate::secret::acl;
    chat("hosb-me", Some(70));
    let drafts = drafts_at_start("hosb-me").unwrap();
    let private = crate::secret::private_sddl(&acl::user_sid().unwrap());
    let cap = "S-1-5-21-1111111111-2222222222-3333333333-4444";
    let has = |sid: &str| acl::dacl_sddl(&drafts).unwrap().contains(sid);
    acl::set_protected(&drafts, &format!("{private}(A;OICI;0x1301bf;;;BU)(A;OICI;0x1301bf;;;{cap})")).unwrap();
    assert_eq!(drafts_at_start("hosb-me").unwrap(), drafts);
    assert!(acl::is_private(&drafts).unwrap() && !has("BU") && !has(cap), "{}", acl::dacl_sddl(&drafts).unwrap());
    // The sandbox's grant, added after the start: kept.
    acl::set_protected(&drafts, &format!("{private}(A;OICI;0x1301bf;;;{cap})")).unwrap();
    assert_eq!(drafts_of("hosb-me").unwrap(), drafts);
    let (failed, how) = call("hosb-me", json!({}));
    assert!(!failed && how.contains(&*drafts.to_string_lossy()), "{how}");
    assert!(has(cap), "{}", acl::dacl_sddl(&drafts).unwrap());
    // A junction in its place: refused at a look, and at the start.
    std::fs::remove_dir_all(&drafts).unwrap();
    let elsewhere = std::env::temp_dir().join(format!("luna-handoff-sb-elsewhere-{}", std::process::id()));
    std::fs::create_dir_all(&elsewhere).unwrap();
    assert!(file::tests::dir_link(&elsewhere, &drafts));
    assert!(drafts_of("hosb-me").is_err() && drafts_at_start("hosb-me").is_err());
    let _ = std::fs::remove_dir(&drafts);
    let _ = std::fs::remove_dir_all(&elsewhere);
    let _ = std::fs::remove_dir_all(drafts.parent().unwrap());
    forget_session("hosb-me");
}
#[test]
fn the_chats_own_drafts_folder_is_where_it_writes_and_what_is_taken() {
    chat("hodr-me", Some(70));
    chat("hodr-other", Some(70));
    // A link put at a name a draft could have, before the folder was Luna's
    // alone, where links can be made here: never the file named.
    let drafts = drafts_of("hodr-me").unwrap();
    let elsewhere = std::env::temp_dir().join(format!("luna-handoff-dr-target-{}.md", std::process::id()));
    std::fs::write(&elsewhere, "theirs").unwrap();
    #[cfg(windows)]
    let _ = std::os::windows::fs::symlink_file(&elsewhere, drafts.join("handoff.md"));
    #[cfg(unix)]
    let _ = std::os::unix::fs::symlink(&elsewhere, drafts.join("handoff.md"));
    // Named by the answer: a name of its own in its drafts folder, not there
    // yet, for its CLI to make the file at (Codex's Windows sandbox could not
    // write one Luna made first, private).
    let (failed, how) = call("hodr-me", json!({}));
    let at = drafts.to_string_lossy().into_owned();
    let named = how.split(&*at).nth(1).and_then(|rest| rest.split(", in a folder").next()).expect("a file in the drafts folder");
    let draft = PathBuf::from(format!("{at}{named}"));
    assert!(!failed && draft != drafts.join("handoff.md") && !how.contains("writes no file"), "{how}");
    assert_eq!(std::fs::symlink_metadata(&draft).unwrap_err().kind(), std::io::ErrorKind::NotFound, "{}", draft.display());
    let (_, again) = call("hodr-me", json!({}));
    assert!(!again.contains(&*draft.to_string_lossy()), "each answer names another");
    std::fs::write(&draft, "# Goal\nShip it.\n").unwrap();
    assert_eq!(std::fs::read_to_string(&elsewhere).unwrap(), "theirs", "nothing written through the link");
    let _ = std::fs::remove_file(&elsewhere);
    assert_eq!(schedule("hodr-me", Provider::Claude, &draft.to_string_lossy(), before()).unwrap()["n"], 1);
    // Another chat's drafts are Luna's folder to it.
    let e = schedule("hodr-other", Provider::Claude, &draft.to_string_lossy(), before()).unwrap_err();
    assert!(e.contains("Luna's own folder"), "{e}");
    // A chat that can write no file is told so, not sent to write one.
    register("hodr-ro", Provider::Codex, "C:/p", "C:/acc/openai/two", None, true, json!({ "sandbox": "read-only" }));
    ask("hodr-ro", store::AskKind::Manual);
    let (failed, how) = call("hodr-ro", json!({}));
    assert!(!failed && how.contains("Codex's read-only sandbox, which writes no file"), "{how}");
    finish("hodr-me", 1, Ended::Done, &String::new);
    for id in ["hodr-me", "hodr-other", "hodr-ro"] {
        forget_session(id);
    }
}

#[test]
fn a_drafts_folder_its_start_could_not_make_private_is_neither_named_nor_taken_from() {
    chat("hodrf-me", Some(70));
    // Something not a folder at its name: the start cannot make it private.
    let drafts = copies_of("hodrf-me").unwrap().join(DRAFTS);
    std::fs::write(&drafts, "not a folder").unwrap();
    let why = drafts_at_start("hodrf-me").unwrap_err();
    assert_eq!(drafts_refused("hodrf-me").as_deref(), Some(why.as_str()));
    // The chat is sent to write its handoff outside Luna's folder.
    let (failed, how) = call("hodrf-me", json!({}));
    assert!(!failed && how.contains("of your own, outside Luna's folder: in your workspace, say."), "{how}");
    assert!(!how.contains(&*drafts.to_string_lossy()), "{how}");
    // Nor is a file taken from a folder made at that name since.
    std::fs::remove_file(&drafts).unwrap();
    std::fs::create_dir(&drafts).unwrap();
    let draft = drafts.join("handoff-0123abcd.md");
    std::fs::write(&draft, "# Goal\nShip it.\n").unwrap();
    let e = schedule("hodrf-me", Provider::Claude, &draft.to_string_lossy(), before()).unwrap_err();
    assert!(e.contains("could not be made private at its start") && e.contains(&why), "{e}");
    // The next start that makes it private puts it back in use.
    drafts_at_start("hodrf-me").unwrap();
    assert_eq!(drafts_refused("hodrf-me"), None);
    let (_, how) = call("hodrf-me", json!({}));
    assert!(how.contains(&*drafts.to_string_lossy()), "{how}");
    // A chat forgotten is forgotten here too.
    std::fs::remove_dir_all(&drafts).unwrap();
    std::fs::write(&drafts, "not a folder").unwrap();
    assert!(drafts_at_start("hodrf-me").is_err() && drafts_refused("hodrf-me").is_some());
    forget_session("hodrf-me");
    assert_eq!(drafts_refused("hodrf-me"), None);
}

#[test]
fn a_handoff_written_by_the_firm_notice_alone_records_its_checklist() {
    chat("hochk-me", Some(70));
    let (dir, file) = scratch_file("chk");
    let told = |level| store::Told { conversation: "c".into(), level, floor: 0, asked: vec![91], compacted: 0, tokens: 182_000 };
    // The first notice gives nothing to write by; the firm one its checklist.
    store::change("hochk-me", |r| told_of(r, told(1), false)).unwrap();
    assert!(store::records()["hochk-me"].served.is_none());
    store::change("hochk-me", |r| told_of(r, told(2), true)).unwrap();
    let given = store::records()["hochk-me"].served.clone().expect("the checklist, as given");
    assert_eq!(given, served::Served::checklist());
    // Written by it without the tool: on the switch, and on the handoff's record.
    let v = schedule("hochk-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    assert!(v["missing"].as_array().is_some_and(|m| !m.is_empty()), "Luna's checklist is looked for: {v}");
    assert_eq!(store::records()["hochk-me"].switch.as_ref().unwrap().instructions.as_ref(), Some(&given));
    finish("hochk-me", 1, Ended::Done, &String::new);
    assert_eq!(store::records()["hochk-me"].past[0].instructions.as_ref(), Some(&given));
    // Given the whole instructions first, a firm notice does not take their place.
    ask("hochk-me", store::AskKind::Auto);
    let (failed, _) = call("hochk-me", json!({}));
    assert!(!failed);
    store::change("hochk-me", |r| told_of(r, told(2), true)).unwrap();
    assert_eq!(store::records()["hochk-me"].served.as_ref().map(|s| s.source), Some(served::Given::Luna));
    done_with(&["hochk-me"], &dir);
}

#[test]
fn the_tool_without_a_file_answers_how_and_the_handoff_keeps_which_text() {
    chat("hoins-me", Some(70));
    let (dir, file) = scratch_file("ins");
    let (failed, how) = call("hoins-me", json!({}));
    assert!(!failed, "{how}");
    let own = crate::library::handoff_instructions();
    for want in [
        "1. Let your CLI's own background jobs",
        "3. Call luna_session_handoff with file",
        "4. End your turn",
        "leave them out of the file",
        &format!("(Luna's own, {}, sha256 {})", env!("CARGO_PKG_VERSION"), own.short_sha()),
        own.body(),
    ] {
        assert!(how.contains(want), "{want}:\n{how}");
    }
    assert!(!how.contains("name: session-handoff"), "front matter is not read out");
    let served = store::records()["hoins-me"].served.clone().expect("what it was served is kept");
    assert_eq!((served.source, served.sha256.as_str()), (served::Given::Luna, own.sha256.as_str()));
    // An empty file is no file: the same answer, nothing scheduled.
    let (failed, again) = call("hoins-me", json!({ "file": " " }));
    assert!(!failed && again.contains("How to hand this chat off") && !holds_text("hoins-me"));
    // The handoff written by it keeps it, through to its record.
    schedule("hoins-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    assert_eq!(store::records()["hoins-me"].switch.as_ref().unwrap().instructions.as_ref(), Some(&served));
    let (failed, e) = call("hoins-me", json!({}));
    assert!(failed && e.contains("under way"), "{e}");
    finish("hoins-me", 1, Ended::Done, &String::new);
    let rec = store::records()["hoins-me"].clone();
    assert_eq!(rec.past[0].instructions.as_ref(), Some(&served));
    assert!(rec.served.is_none(), "let go once the handoff went through");
    done_with(&["hoins-me"], &dir);
}

#[test]
fn a_thin_file_written_to_luna_s_instructions_is_named_not_refused() {
    chat("hothin-me", Some(70));
    let (dir, file) = scratch_file("thin");
    // "# Objective" is no heading of the checklist: every section of it is named.
    let v = schedule("hothin-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    assert_eq!(v["scheduled"], true);
    let missing: Vec<&str> = v["missing"].as_array().unwrap().iter().map(|m| m.as_str().unwrap()).collect();
    assert_eq!(missing, ["Goal", "Next action", "The user's words", "State", "Pending", "Decisions and dead ends"]);
    assert!(v["note"].as_str().unwrap().contains("goes as it is"));
    finish("hothin-me", 1, Ended::Done, &String::new);
    ask("hothin-me", store::AskKind::Auto);
    // Written to an override: its sections are its author's, none looked for.
    store::change("hothin-me", |r| {
        r.served = Some(served::Served {
            source: served::Given::Library,
            sha256: "ab".into(),
            version: None,
            path: Some("/lib/system/session-handoff/SKILL.md".into()),
        })
    })
    .unwrap();
    let v = schedule("hothin-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    assert!(v.get("missing").is_none() && v.get("note").is_none(), "{v}");
    done_with(&["hothin-me"], &dir);
}

#[test]
fn a_chat_asked_and_not_handing_off_shows_it_until_a_handoff_goes_through() {
    chat("hoask-me", Some(70));
    chat("hoask-off", None);
    let (dir, file) = scratch_file("ask");
    assert_eq!(asked_of("hoask-me"), None, "never asked");
    let told = store::Told { conversation: "c".into(), level: 2, floor: 0, asked: vec![71, 86], compacted: 1, tokens: 172_000 };
    store::change("hoask-me", |r| r.told = Some(told.clone())).unwrap();
    assert_eq!(asked_of("hoask-me"), Some(HandoffAsked { at: vec![71, 86], compacted: 1 }));
    // Asked of a chat that no longer has its threshold: nothing to show.
    store::change("hoask-off", |r| r.told = Some(told.clone())).unwrap();
    assert_eq!(asked_of("hoask-off"), None);
    // Under way, then through: no longer "not done".
    schedule("hoask-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    assert_eq!(asked_of("hoask-me"), None);
    finish("hoask-me", 1, Ended::Done, &String::new);
    assert_eq!(asked_of("hoask-me"), None);
    assert!(store::records()["hoask-me"].told.as_ref().is_some_and(|t| t.asked.is_empty() && t.compacted == 0));
    // A handoff given up leaves the asks standing.
    store::change("hoask-me", |r| r.told = Some(told.clone())).unwrap();
    ask("hoask-me", store::AskKind::Auto);
    schedule("hoask-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    finish("hoask-me", 2, Ended::Failed { why: "no rest".into(), reset: false }, &String::new);
    assert_eq!(asked_of("hoask-me").map(|a| a.at), Some(vec![71, 86]));
    reg().lock().unwrap().news.retain(|(c, _)| c != "hoask-me");
    done_with(&["hoask-me", "hoask-off"], &dir);
}

#[test]
fn the_panel_names_the_instructions_and_skills_of_the_same_name() {
    let base = std::env::temp_dir().join(format!("luna-handoff-same-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (folder, account) = (base.join("repo"), base.join("acc"));
    let skill = |root: &Path| {
        let p = root.join("session-handoff").join("SKILL.md");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "---\nname: session-handoff\ndescription: theirs\n---\n").unwrap();
        p.to_string_lossy().into_owned()
    };
    std::fs::create_dir_all(&account).unwrap();
    assert!(served::same_named(Provider::Claude, &folder, &account).is_empty());
    let claude_project = skill(&folder.join(".claude").join("skills"));
    let codex_project = skill(&folder.join(".agents").join("skills"));
    let in_account = skill(&account.join("skills"));
    // Each CLI's own place in the project, and the account's for both.
    assert_eq!(served::same_named(Provider::Claude, &folder, &account), [claude_project, in_account.clone()]);
    assert_eq!(served::same_named(Provider::Codex, &folder, &account), [codex_project, in_account]);
    // The panel's word: for a chat listed the tool, what it would be served now.
    let (folder_s, account_s) = (folder.to_string_lossy().into_owned(), account.to_string_lossy().into_owned());
    register("hopan-me", Provider::Claude, &folder_s, &account_s, None, true, json!({ "handoff": 70 }));
    register("hopan-off", Provider::Claude, &folder_s, &account_s, None, true, json!({}));
    register("hopan-plain", Provider::Claude, &folder_s, &account_s, None, false, json!({ "handoff": 70 }));
    let shown = applied_of("hopan-me").expect("a chat with it on");
    assert_eq!(shown.same_named.len(), 2);
    assert_eq!(shown.instructions, crate::library::handoff_instructions().state());
    assert_eq!(crate::applied::applied_by_luna("hopan-me".into()).handoff, Some(shown), "in the panel, started or not");
    // Listed the tool, the threshold set or not; nothing without Luna's tools.
    assert_eq!(applied_of("hopan-off").map(|a| a.same_named.len()), Some(2));
    assert!(applied_of("hopan-plain").is_none() && crate::applied::applied_by_luna("hopan-plain".into()).handoff.is_none());
    for id in ["hopan-me", "hopan-off", "hopan-plain"] {
        forget_session(id);
    }
    let _ = std::fs::remove_dir_all(&base);
}
