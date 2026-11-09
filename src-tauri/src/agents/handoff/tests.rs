//! The handoff as the rest of Luna meets it: who is offered the tool, whose
//! chat a call can touch, what waits while one is under way, and what a
//! chat's deletion takes with it. The machine itself is tested in phase.rs,
//! the file in file.rs, the words in facts.rs, the save in store.rs.

use super::*;
use crate::agents::registry::register;
use crate::agents::teardown::forget as forget_session;
use serde_json::json;

const ACC: &str = "C:/acc/anthropic/one";

/// A chat of the person's with Luna's tools, auto-handoff at `threshold`, and
/// no copies on disk from a run before: tests share the data dir, and one that
/// stopped half way leaves its folder.
fn chat(id: &str, threshold: Option<u8>) {
    if let Ok(dir) = dir_of(id) {
        let _ = std::fs::remove_dir_all(dir);
    }
    let settings = match threshold {
        Some(t) => json!({ "model": "opus", "handoff": t }),
        None => json!({ "model": "opus" }),
    };
    register(id, Provider::Claude, "C:/p", ACC, None, true, settings);
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
fn the_tool_is_listed_only_to_a_persons_chat_with_auto_handoff_on() {
    chat("hohv-on", Some(70));
    chat("hohv-off", None);
    register("hohv-kid", Provider::Claude, "C:/p", ACC, Some("hohv-on"), true, json!({ "handoff": 70 }));
    register("hohv-plain", Provider::Claude, "C:/p", ACC, None, false, json!({ "handoff": 70 }));
    chat("hohv-odd", Some(99));
    assert!(listed_names("hohv-on").contains(&"luna_session_handoff".to_string()));
    assert!(is_listed("hohv-on"), "its CLI has it now");
    for id in ["hohv-off", "hohv-kid", "hohv-odd"] {
        let names = listed_names(id);
        assert!(names.contains(&"luna_session_list".to_string()), "{id} has the rest");
        assert!(!names.contains(&"luna_session_handoff".to_string()), "{id}");
    }
    assert!(listed_names("hohv-plain").is_empty(), "no tools: nothing, not even this one");
    // Started again, its CLI lists them anew.
    chat("hohv-on", Some(70));
    assert!(!is_listed("hohv-on"));
    for id in ["hohv-on", "hohv-off", "hohv-kid", "hohv-plain", "hohv-odd"] {
        forget_session(id);
    }
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
    let (failed, e) = call("hont-me", json!({}));
    assert!(failed && e.contains("file is required"), "{e}");
    // Not offered: refused, whoever's file it names.
    let (failed, e) = call("hont-off", json!({ "file": "C:/x/h.md" }));
    assert!(failed && e.contains("has it off"), "{e}");
    let (failed, e) = call("hont-kid", json!({ "file": "C:/x/h.md" }));
    assert!(failed && e.contains("helper"), "{e}");
    let (failed, e) = call(DEV_CALLER, json!({ "file": "C:/x/h.md" }));
    assert!(failed, "{e}");
    // Offered, but not running in a terminal here: nothing scheduled.
    let (failed, e) = call("hont-me", json!({ "file": "C:/x/h.md" }));
    assert!(failed && e.contains("not running"), "{e}");
    // What one chat schedules is its own: no other's record moves.
    let (dir, file) = scratch_file("nt");
    schedule("hont-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    assert!(holds_text("hont-me") && !holds_text("hont-other"));
    assert!(store::records().get("hont-other").is_none());
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
    reg().lock().unwrap().news.push(("hosc-me".into(), facts::notice(75, 70, false)));
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
    finish("hosc-me", 1, Ended::Done, "");
    assert!(!holds_text("hosc-me"));
    assert_eq!(mark_of("hosc-me").map(|m| (m.n, m.state)), Some((1, "done")));
    assert_eq!(handoff_file("hosc-me".into(), 1).unwrap(), "# Objective\nFinish the campaign.\n");
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
    finish("hohw-me", 1, Ended::Done, "");
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
        "PROMPT",
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
    finish(
        "hogu-me",
        2,
        Ended::Failed { why: "the new conversation did not take Luna's prompt within 60 s".into(), reset: true },
        "PROMPT",
    );
    let news: Vec<String> = reg().lock().unwrap().news.iter().filter(|(c, _)| c == "hogu-me").map(|(_, l)| l.clone()).collect();
    assert!(news[0].contains("cleared the conversation") && news[0].ends_with("PROMPT"), "{}", news[0]);
    reg().lock().unwrap().news.retain(|(c, _)| c != "hogu-me");
    done_with(&["hogu-me"], &dir);
}

#[test]
fn turning_it_off_gives_up_a_scheduled_handoff_and_the_setting_follows_at_once() {
    chat("hots-me", None);
    assert!(!offers_tool("hots-me"));
    assert!(set_auto_handoff("hots-me".into(), Some(91)).unwrap_err().contains("not one"));
    assert!(set_auto_handoff("hots-me".into(), Some(0)).is_err());
    // On: the registry has it now; no CLI runs here, so nothing to resume.
    assert_eq!(set_auto_handoff("hots-me".into(), Some(60)), Ok(AutoHandoff { resume: false }));
    assert!(offers_tool("hots-me"));
    assert_eq!(crate::agents::settings_of("hots-me")["handoff"], 60);
    let (dir, file) = scratch_file("ts");
    schedule("hots-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    // Off: the tool refuses from now, and what was only scheduled is given up.
    set_auto_handoff("hots-me".into(), None).unwrap();
    assert!(!offers_tool("hots-me") && crate::agents::settings_of("hots-me").get("handoff").is_none());
    assert!(!holds_text("hots-me"));
    assert_eq!(mark_of("hots-me").and_then(|m| m.why), Some("auto-handoff was turned off for this chat".into()));
    let told: Vec<String> = reg().lock().unwrap().news.iter().filter(|(c, _)| c == "hots-me").map(|(_, l)| l.clone()).collect();
    assert!(told[0].contains("refuses until the person turns it on"), "not asked to call a tool it no longer has: {}", told[0]);
    reg().lock().unwrap().news.retain(|(c, _)| c != "hots-me");
    done_with(&["hots-me"], &dir);
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
    register("hogf-kid", Provider::Codex, "C:/p", "C:/acc/openai/two", Some("hogf-me"), false, json!({ "background": true }));
    reg().lock().unwrap().owed.insert(("hogf-me".into(), "hogf-kid".into()), crate::agents::owed::Owed::from(0, 0));
    let f = gather("hogf-me");
    assert_eq!(f.helpers.len(), 1);
    assert_eq!((f.helpers[0].id.as_str(), f.helpers[0].owes, f.helpers[0].account.as_str()), ("hogf-kid", true, "codex/two"));
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
    assert_eq!(phase::run(&t, "hocr-me", 1, "prompt", &Stored), Ended::Back);
    assert!(t.typed.borrow().is_empty(), "no /clear with no handoff to follow it");
    assert_eq!(mark_of("hocr-me").map(|m| m.state), Some("failed"));
    // A switch that has taken its reset is not given up by turning it off: the
    // prompt is still to follow.
    set_auto_handoff("hocr-me".into(), Some(70)).unwrap();
    schedule("hocr-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    Stored.advance("hocr-me", 2, Phase::Scheduled, Phase::Resetting { at: 1 }, 1).unwrap();
    set_auto_handoff("hocr-me".into(), None).unwrap();
    assert!(holds_text("hocr-me"), "the switch goes on to its prompt");
    finish("hocr-me", 2, Ended::Done, "");
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
    let ended = phase::run(&t, "hods-me", 2, "prompt", &Stored);
    assert!(t.typed.borrow().is_empty());
    assert!(matches!(&ended, Ended::Failed { why, reset: false } if why.contains("could not be saved")), "{ended:?}");
    finish("hods-me", 2, ended, "prompt");
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
    finish("hoex-me", 1, Ended::Done, "");
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
