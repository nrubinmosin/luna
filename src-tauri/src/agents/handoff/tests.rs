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
    // Without a file it only answers how: nothing is scheduled.
    let (failed, how) = call("hont-me", json!({}));
    assert!(!failed && how.contains("How to hand this chat off") && !holds_text("hont-me"), "{how}");
    // Not offered: refused as a tool there is not, whoever's file it names.
    for id in ["hont-off", "hont-kid"] {
        let (failed, e) = call(id, json!({ "file": "C:/x/h.md" }));
        assert!(failed && e.contains("unknown tool luna_session_handoff"), "{id}: {e}");
    }
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
    // Or the chat starts again without it as the file is read: the same.
    set_auto_handoff("hooff-me".into(), Some(70)).unwrap();
    file::on_read(&path, || {
        register("hooff-me", Provider::Claude, "C:/p", ACC, None, true, json!({ "model": "opus" }));
    });
    let e = schedule("hooff-me", Provider::Claude, &path, before()).unwrap_err();
    assert!(e.contains("turned off for this chat while the file was copied"), "{e}");
    assert!(!holds_text("hooff-me"));
    reg().lock().unwrap().news.retain(|(c, _)| c != "hooff-me");
    done_with(&["hooff-me"], &dir);
}

/// Only a person's chat with auto-handoff on knows of it: a helper, a chat
/// without Luna's tools, one with them but without the tick, and the dev caller
/// are not offered the tool, are told nothing of it at their start, find no
/// topic of it in the guide, and calling it all the same are refused as for a
/// tool there is not, with nothing kept for them.
#[test]
fn nobody_but_a_chat_with_auto_handoff_on_hears_of_it() {
    chat("hoaud-me", Some(70));
    chat("hoaud-tools", None);
    register("hoaud-kid", Provider::Claude, "C:/p", ACC, Some("hoaud-me"), true, json!({ "handoff": 70 }));
    register("hoaud-plain", Provider::Claude, "C:/p", ACC, None, false, json!({ "handoff": 70 }));
    let tells = |text: &str| {
        let t = text.to_lowercase();
        ["auto-handoff", "session-handoff", "what luna applied", "takes: file", "hand this chat off"].iter().any(|w| t.contains(w))
    };
    for id in ["hoaud-tools", "hoaud-kid", "hoaud-plain", DEV_CALLER] {
        assert!(!listed_names(id).iter().any(|n| n.contains("handoff")), "{id}");
        let init = crate::mcp::handle(id, r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#).unwrap();
        assert!(!init.to_string().to_lowercase().contains("handoff"), "{id}: {init}");
        for args in [json!({}), json!({ "file": "C:/x/h.md" }), json!({ "chat": "hoaud-me" })] {
            let (failed, e) = call(id, args.clone());
            assert!(failed && !tells(&e), "{id} {args}: {e}");
        }
        let guide = json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": "luna_guide_get", "arguments": { "topic": "luna_session_handoff" } } });
        let r = crate::mcp::handle(id, &guide.to_string()).unwrap();
        assert_eq!(r["result"]["isError"], true, "{id}: no such topic");
        assert!(!holds_text(id) && store::records().get(id).is_none(), "{id}: nothing kept for it");
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
    // The chat that has it on: the tool, and how to write by.
    assert!(listed_names("hoaud-me").contains(&"luna_session_handoff".to_string()));
    let (failed, how) = call("hoaud-me", json!({}));
    assert!(!failed && how.contains("How to hand this chat off"), "{how}");
    for id in ["hoaud-me", "hoaud-tools", "hoaud-kid", "hoaud-plain"] {
        forget_session(id);
    }
}

#[test]
fn the_chats_own_drafts_folder_is_where_it_writes_and_what_is_taken() {
    chat("hodr-me", Some(70));
    chat("hodr-other", Some(70));
    // Named by the answer, made private for it; a file there is taken.
    let (failed, how) = call("hodr-me", json!({}));
    let drafts = dir_of("hodr-me").unwrap().join("drafts");
    let draft = drafts.join("handoff.md");
    assert!(!failed && how.contains(&*draft.to_string_lossy()) && drafts.is_dir(), "{how}");
    assert!(!how.contains("writes no file"), "this one writes: {how}");
    std::fs::write(&draft, "# Goal\nShip it.\n").unwrap();
    assert_eq!(schedule("hodr-me", Provider::Claude, &draft.to_string_lossy(), before()).unwrap()["n"], 1);
    // Another chat's drafts are Luna's folder to it.
    let e = schedule("hodr-other", Provider::Claude, &draft.to_string_lossy(), before()).unwrap_err();
    assert!(e.contains("Luna's own folder"), "{e}");
    // A chat that can write no file is told so, not sent to write one.
    register("hodr-ro", Provider::Codex, "C:/p", "C:/acc/openai/two", None, true, json!({ "sandbox": "read-only", "handoff": 70 }));
    let (failed, how) = call("hodr-ro", json!({}));
    assert!(!failed && how.contains("Codex's read-only sandbox, which writes no file"), "{how}");
    finish("hodr-me", 1, Ended::Done, &String::new);
    for id in ["hodr-me", "hodr-other", "hodr-ro"] {
        forget_session(id);
    }
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
    // Asked of a chat that no longer has it on: nothing to show.
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
    // The panel's word: for a chat with it on, what it would be served now.
    let (folder_s, account_s) = (folder.to_string_lossy().into_owned(), account.to_string_lossy().into_owned());
    register("hopan-me", Provider::Claude, &folder_s, &account_s, None, true, json!({ "handoff": 70 }));
    register("hopan-off", Provider::Claude, &folder_s, &account_s, None, true, json!({}));
    let shown = applied_of("hopan-me").expect("a chat with it on");
    assert_eq!(shown.same_named.len(), 2);
    assert_eq!(shown.instructions, crate::library::handoff_instructions().state());
    assert_eq!(crate::applied::applied_by_luna("hopan-me".into()).handoff, Some(shown), "in the panel, started or not");
    assert!(applied_of("hopan-off").is_none() && crate::applied::applied_by_luna("hopan-off".into()).handoff.is_none());
    forget_session("hopan-me");
    forget_session("hopan-off");
    let _ = std::fs::remove_dir_all(&base);
}
