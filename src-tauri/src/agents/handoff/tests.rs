//! The handoff as the rest of Luna meets it: who is offered the tool, whose
//! chat a call can touch, what waits while one is under way, and what a
//! chat's deletion takes with it. The machine itself is tested in phase.rs,
//! the file in file.rs, the words in facts.rs, the save in store.rs.

use super::*;
use crate::agents::registry::register;
use crate::agents::teardown::forget as forget_session;
use serde_json::json;

const ACC: &str = "C:/acc/anthropic/one";

/// A chat of the person's with Luna's tools, auto-handoff at `threshold`.
fn chat(id: &str, threshold: Option<u8>) {
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
    chat("hv-on", Some(70));
    chat("hv-off", None);
    register("hv-kid", Provider::Claude, "C:/p", ACC, Some("hv-on"), true, json!({ "handoff": 70 }));
    register("hv-plain", Provider::Claude, "C:/p", ACC, None, false, json!({ "handoff": 70 }));
    chat("hv-odd", Some(99));
    assert!(listed_names("hv-on").contains(&"luna_session_handoff".to_string()));
    assert!(is_listed("hv-on"), "its CLI has it now");
    for id in ["hv-off", "hv-kid", "hv-odd"] {
        let names = listed_names(id);
        assert!(names.contains(&"luna_session_list".to_string()), "{id} has the rest");
        assert!(!names.contains(&"luna_session_handoff".to_string()), "{id}");
    }
    assert!(listed_names("hv-plain").is_empty(), "no tools: nothing, not even this one");
    // Started again, its CLI lists them anew.
    chat("hv-on", Some(70));
    assert!(!is_listed("hv-on"));
    for id in ["hv-on", "hv-off", "hv-kid", "hv-plain", "hv-odd"] {
        forget_session(id);
    }
}

#[test]
fn a_call_reaches_only_the_callers_own_chat() {
    chat("nt-me", Some(70));
    chat("nt-other", Some(70));
    chat("nt-off", None);
    register("nt-kid", Provider::Claude, "C:/p", ACC, Some("nt-me"), true, json!({ "handoff": 70 }));
    // No argument names a chat: one that tries is refused before anything runs.
    for key in ["id", "target", "chat", "scope"] {
        let (failed, e) = call("nt-me", json!({ "file": "C:/x/h.md", key: "nt-other" }));
        assert!(failed && e.contains(&format!("no argument `{key}`")) && e.contains("takes: file"), "{key}: {e}");
    }
    let (failed, e) = call("nt-me", json!({}));
    assert!(failed && e.contains("file is required"), "{e}");
    // Not offered: refused, whoever's file it names.
    let (failed, e) = call("nt-off", json!({ "file": "C:/x/h.md" }));
    assert!(failed && e.contains("has it off"), "{e}");
    let (failed, e) = call("nt-kid", json!({ "file": "C:/x/h.md" }));
    assert!(failed && e.contains("helper"), "{e}");
    let (failed, e) = call(DEV_CALLER, json!({ "file": "C:/x/h.md" }));
    assert!(failed, "{e}");
    // Offered, but not running in a terminal here: nothing scheduled.
    let (failed, e) = call("nt-me", json!({ "file": "C:/x/h.md" }));
    assert!(failed && e.contains("not running"), "{e}");
    // What one chat schedules is its own: no other's record moves.
    let (dir, file) = scratch_file("nt");
    schedule("nt-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    assert!(holds_text("nt-me") && !holds_text("nt-other"));
    assert!(store::records().get("nt-other").is_none());
    done_with(&["nt-me", "nt-other", "nt-off", "nt-kid"], &dir);
}

#[test]
fn a_call_is_refused_while_the_clis_own_jobs_run_naming_them() {
    chat("oj-me", Some(70));
    crate::activity::subagents_for_test("oj-me", 2);
    let (failed, e) = call("oj-me", json!({ "file": "C:/x/h.md" }));
    assert!(failed && e.contains("2 subagents") && e.contains("cannot cross"), "{e}");
    assert!(!holds_text("oj-me"), "nothing scheduled");
    crate::activity::subagents_for_test("oj-me", 0);
    crate::activity::forget("oj-me");
    forget_session("oj-me");
}

#[test]
fn a_schedule_copies_numbers_and_refuses_a_second_under_way() {
    chat("sc-me", Some(70));
    let (dir, file) = scratch_file("sc");
    // A threshold notice not yet typed goes: the new conversation has nothing to hand off.
    reg().lock().unwrap().news.push(("sc-me".into(), facts::notice(75, 70, false)));
    reg().lock().unwrap().news.push(("sc-me".into(), "Your helper h1 answered.".into()));
    let v = schedule("sc-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    assert_eq!((v["scheduled"].clone(), v["n"].clone()), (json!(true), json!(1)));
    let snapshot = PathBuf::from(v["snapshot"].as_str().unwrap());
    assert_eq!(snapshot, dir_of("sc-me").unwrap().join("1.md"));
    assert_eq!(std::fs::read_to_string(&snapshot).unwrap(), "# Objective\nFinish the campaign.\n");
    let news: Vec<String> = reg().lock().unwrap().news.iter().filter(|(c, _)| c == "sc-me").map(|(_, l)| l.clone()).collect();
    assert_eq!(news, ["Your helper h1 answered."]);
    let e = schedule("sc-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap_err();
    assert!(e.contains("handoff 1 is under way"), "{e}");
    assert_eq!(mark_of("sc-me").map(|m| (m.n, m.state)), Some((1, "underWay")));
    // Through: on record, and the next is number 2.
    finish("sc-me", Ended::Done, "");
    assert!(!holds_text("sc-me"));
    assert_eq!(mark_of("sc-me").map(|m| (m.n, m.state)), Some((1, "done")));
    assert_eq!(handoff_file("sc-me".into(), 1).unwrap(), "# Objective\nFinish the campaign.\n");
    let v = schedule("sc-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    assert_eq!(v["n"], 2);
    reg().lock().unwrap().news.retain(|(c, _)| c != "sc-me");
    done_with(&["sc-me"], &dir);
}

#[test]
fn notices_and_sends_wait_while_a_handoff_is_under_way() {
    chat("hw-me", Some(70));
    chat("hw-sender", None);
    let (dir, file) = scratch_file("hw");
    assert!(!crate::agents::notifier::notice_waits("hw-me"));
    schedule("hw-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    assert!(crate::agents::notifier::notice_waits("hw-me"), "the notifier holds what it has for the chat");
    // Text another session sends goes in after, into the new conversation.
    let sent = crate::agents::send("hw-sender", "hw-me", "the build is green", false).unwrap();
    assert!(sent.held && !sent.started);
    assert!(crate::agents::held::is_held("hw-sender", "hw-me"));
    // A command is still refused as one, held or not.
    assert!(crate::agents::send("hw-sender", "hw-me", "/exit", false).is_err());
    finish("hw-me", Ended::Done, "");
    assert!(!crate::agents::notifier::notice_waits("hw-me"), "released once it is through");
    reg().lock().unwrap().held.retain(|h| h.id != "hw-me");
    done_with(&["hw-me", "hw-sender"], &dir);
}

#[test]
fn a_handoff_given_up_before_its_reset_tells_the_old_conversation() {
    chat("gu-me", Some(70));
    let (dir, file) = scratch_file("gu");
    schedule("gu-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    finish(
        "gu-me",
        Ended::Failed { why: "the CLI did not start a new conversation within 60 s of the reset".into(), reset: false },
        "PROMPT",
    );
    assert!(!holds_text("gu-me"), "released");
    let news: Vec<String> = reg().lock().unwrap().news.iter().filter(|(c, _)| c == "gu-me").map(|(_, l)| l.clone()).collect();
    assert_eq!(news.len(), 1);
    assert!(news[0].contains("did not go through") && news[0].contains("goes on as it was") && !news[0].contains("PROMPT"), "{}", news[0]);
    assert!(news[0].contains("Call luna_session_handoff again"), "the tool is still its: {}", news[0]);
    assert_eq!(mark_of("gu-me").map(|m| (m.state, m.why.is_some())), Some(("failed", true)));
    // After the reset, the new conversation hears where to start.
    reg().lock().unwrap().news.retain(|(c, _)| c != "gu-me");
    schedule("gu-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    finish("gu-me", Ended::Failed { why: "the new conversation did not take Luna's prompt within 60 s".into(), reset: true }, "PROMPT");
    let news: Vec<String> = reg().lock().unwrap().news.iter().filter(|(c, _)| c == "gu-me").map(|(_, l)| l.clone()).collect();
    assert!(news[0].contains("cleared the conversation") && news[0].ends_with("PROMPT"), "{}", news[0]);
    reg().lock().unwrap().news.retain(|(c, _)| c != "gu-me");
    done_with(&["gu-me"], &dir);
}

#[test]
fn turning_it_off_gives_up_a_scheduled_handoff_and_the_setting_follows_at_once() {
    chat("ts-me", None);
    assert!(!offers_tool("ts-me"));
    assert!(set_auto_handoff("ts-me".into(), Some(91)).unwrap_err().contains("not one"));
    assert!(set_auto_handoff("ts-me".into(), Some(0)).is_err());
    // On: the registry has it now; no CLI runs here, so nothing to resume.
    assert_eq!(set_auto_handoff("ts-me".into(), Some(60)), Ok(AutoHandoff { resume: false }));
    assert!(offers_tool("ts-me"));
    assert_eq!(crate::agents::settings_of("ts-me")["handoff"], 60);
    let (dir, file) = scratch_file("ts");
    schedule("ts-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    // Off: the tool refuses from now, and what was only scheduled is given up.
    set_auto_handoff("ts-me".into(), None).unwrap();
    assert!(!offers_tool("ts-me") && crate::agents::settings_of("ts-me").get("handoff").is_none());
    assert!(!holds_text("ts-me"));
    assert_eq!(mark_of("ts-me").and_then(|m| m.why), Some("auto-handoff was turned off for this chat".into()));
    let told: Vec<String> = reg().lock().unwrap().news.iter().filter(|(c, _)| c == "ts-me").map(|(_, l)| l.clone()).collect();
    assert!(told[0].contains("refuses until the person turns it on"), "not asked to call a tool it no longer has: {}", told[0]);
    reg().lock().unwrap().news.retain(|(c, _)| c != "ts-me");
    done_with(&["ts-me"], &dir);
}

#[test]
fn a_chat_deleted_takes_its_handoffs_with_it() {
    chat("fg-me", Some(70));
    let (dir, file) = scratch_file("fg");
    schedule("fg-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    let kept = dir_of("fg-me").unwrap();
    assert!(kept.join("1.md").is_file());
    forget_session("fg-me");
    assert!(!kept.exists() && store::records().get("fg-me").is_none() && !holds_text("fg-me"));
    assert!(dir_of("../x").is_err() && dir_of("a/b").is_err() && dir_of("").is_err(), "only a plain id is made a path");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_facts_name_the_chats_helpers_and_what_they_owe() {
    chat("gf-me", Some(70));
    register("gf-kid", Provider::Codex, "C:/p", "C:/acc/openai/two", Some("gf-me"), false, json!({ "background": true }));
    reg().lock().unwrap().owed.insert(("gf-me".into(), "gf-kid".into()), crate::agents::owed::Owed::from(0, 0));
    let f = gather("gf-me");
    assert_eq!(f.helpers.len(), 1);
    assert_eq!((f.helpers[0].id.as_str(), f.helpers[0].owes, f.helpers[0].account.as_str()), ("gf-kid", true, "codex/two"));
    assert_eq!(f.threshold, Some(70));
    assert!(f.owed_by_others.is_empty());
    forget_session("gf-kid");
    forget_session("gf-me");
}
