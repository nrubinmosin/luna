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
    set_conversation_for_test(id, Some("old"));
    if threshold.is_some() {
        ask(id, store::AskKind::Auto);
    }
}

/// `id` asked for a handoff, by its threshold's notice or the person's
/// request typed, in the conversation its CLI is on (`before`'s).
fn ask(id: &str, kind: store::AskKind) {
    let at = now_ms();
    let typed = (kind == store::AskKind::Manual).then_some(at);
    let asked = store::Asked { typed, conversation: Some("old".into()), ..store::Asked::new(kind, at, None) };
    store::change(id, |r| r.asked = Some(asked)).unwrap();
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
        set_conversation_for_test(id, None);
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

/// The notifier types a failure line later than it is made: one with the
/// continuation in it goes in whole only while the chat is on the
/// conversation Luna's reset opened, and anywhere else, or where Luna cannot
/// tell, as the line naming only the copy.
#[test]
fn a_failure_line_with_the_continuation_goes_only_into_the_conversation_the_reset_opened() {
    chat("hocont-me", Some(70));
    let (dir, file) = scratch_file("cont");
    schedule("hocont-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    store::change("hocont-me", |r| r.switch.as_mut().unwrap().opened = Some("new".into())).unwrap();
    let why = "the new conversation did not take Luna's prompt within 60 s";
    finish("hocont-me", 1, Ended::Failed { why: why.into(), reset: true }, &|| "PROMPT".into());
    let news: Vec<String> = reg().lock().unwrap().news.iter().filter(|(c, _)| c == "hocont-me").map(|(_, l)| l.clone()).collect();
    let line = &news[0];
    assert!(line.ends_with("PROMPT"), "{line}");
    set_conversation_for_test("hocont-me", Some("new"));
    assert_eq!(as_told("hocont-me", line), *line, "on the conversation it was made for: whole");
    let copy = dir_of("hocont-me").unwrap().join("1.md").to_string_lossy().into_owned();
    for elsewhere in [Some("theirs"), None] {
        set_conversation_for_test("hocont-me", elsewhere);
        let short = as_told("hocont-me", line);
        assert!(!short.contains("PROMPT") && short.contains(&copy) && short.contains("Handoff 1") && short.contains(why), "{short}");
    }
    // A line Luna has no record of is told without its prompt; any other as it is.
    let unbound = facts::failed_line(9, "x", "/c/9.md", true, false, "PROMPT");
    set_conversation_for_test("hocont-me", Some("new"));
    assert!(!as_told("hocont-me", &unbound).contains("PROMPT"));
    assert_eq!(as_told("hocont-me", "Your helper h1 answered."), "Your helper h1 answered.");
    reg().lock().unwrap().news.retain(|(c, _)| c != "hocont-me");
    done_with(&["hocont-me"], &dir);
}

/// A move under way keeps the chat from a handoff: a call whose file was
/// being copied as the move began schedules nothing, and the person's ask
/// is neither typed nor closed until the move ends, then looked at again.
#[test]
fn no_handoff_is_scheduled_or_asked_while_a_move_is_under_way() {
    chat("homove-me", None);
    ask("homove-me", store::AskKind::Manual);
    let (dir, file) = scratch_file("homove");
    let shown = moving::tests::a_move(moving::MovePhase::Shown { at: 1 });
    store::change("homove-me", |r| r.moving = Some(shown)).unwrap();
    let e = schedule("homove-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap_err();
    assert!(e.contains("a move of this chat") && e.contains("nothing will switch"), "{e}");
    let r = store::records()["homove-me"].clone();
    assert!(r.switch.is_none() && r.reserved.is_none(), "nothing scheduled, nothing held");
    assert!(!dir_of("homove-me").unwrap().join(format!("{}.md", r.last)).exists(), "its copy gone");
    // Not typed in time, long ago: left as it is while the move is under way.
    store::change("homove-me", |r| r.asked = Some(store::Asked::new(store::AskKind::Manual, 1, None))).unwrap();
    manual_ask_of("homove-me", now_ms());
    assert_eq!(store::records()["homove-me"].asked.as_ref().map(|a| a.kind), Some(store::AskKind::Manual));
    assert!(!reg().lock().unwrap().news.iter().any(|(c, _)| c == "homove-me"), "nothing said to the chat");
    // The move over: looked at again, and withdrawn as undelivered.
    store::change("homove-me", |r| r.moving = Some(moving::tests::a_move(moving::MovePhase::Canceled { at: 2 }))).unwrap();
    manual_ask_of("homove-me", now_ms());
    assert!(store::records()["homove-me"].asked.is_none());
    reg().lock().unwrap().news.retain(|(c, _)| c != "homove-me");
    let _ = store::remove("homove-me");
    done_with(&["homove-me"], &dir);
}

/// The person's `/clear` or `/new` lands between the notifier's look at a
/// failure line with the continuation and its text going in: looked at
/// again once their keys are held, right before the paste, nothing is typed,
/// and the next look tells the conversation they started only of the copy.
/// A threshold notice already put together as a move began is not typed
/// either.
#[test]
fn a_queued_line_is_looked_at_again_under_the_held_keys_before_it_goes_in() {
    chat("hokey-me", Some(70));
    let (dir, file) = scratch_file("keys");
    schedule("hokey-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    store::change("hokey-me", |r| r.switch.as_mut().unwrap().opened = Some("new".into())).unwrap();
    finish("hokey-me", 1, Ended::Failed { why: "x".into(), reset: true }, &|| "PROMPT".into());
    let line = reg().lock().unwrap().news.iter().find(|(c, _)| c == "hokey-me").map(|(_, l)| l.clone()).unwrap();
    set_conversation_for_test("hokey-me", Some("new"));
    crate::pty::fake_terminal_for_test("hokey-me", true);
    let told = as_told("hokey-me", &line);
    assert_eq!(told, line, "as the notifier's look puts it: whole");
    let clear = Box::new(|| set_conversation_for_test("hokey-me", Some("theirs")));
    crate::agents::notifier::deliver_news_for_test("hokey-me", vec![(told, line.clone())], clear);
    let (typed, held) = crate::pty::fake_terminal_of("hokey-me");
    assert_eq!(typed, "", "nothing typed into the conversation the person started");
    assert!(!held, "their keys go on");
    assert!(reg().lock().unwrap().news.iter().any(|(c, l)| c == "hokey-me" && *l == line), "still queued, for the next look");
    assert!(!as_told("hokey-me", &line).contains("PROMPT"), "which names only the copy");
    reg().lock().unwrap().news.retain(|(c, _)| c != "hokey-me");
    // A threshold notice put together before a move began.
    set_conversation_for_test("hokey-me", Some("new"));
    let notice = facts::notice(72, 70, false, true);
    let shown = moving::tests::a_move(moving::MovePhase::Shown { at: 1 });
    let begin = Box::new(move || store::change("hokey-me", |r| r.moving = Some(shown)).unwrap());
    crate::agents::notifier::deliver_news_for_test("hokey-me", vec![(notice.clone(), notice)], begin);
    assert_eq!(crate::pty::fake_terminal_of("hokey-me"), (String::new(), false), "no handoff asked in the middle of a move");
    crate::pty::fake_terminal_for_test("hokey-me", false);
    let _ = store::remove("hokey-me");
    done_with(&["hokey-me"], &dir);
}

/// The person's `/new` sent just before the notifier held their keys is
/// still on its way to the CLI, which reads as the conversation the
/// continuation was made for: a key of theirs since the notifier's look keeps
/// the line back all the same, still queued, and a line bound to no
/// conversation goes in.
#[test]
fn a_continuation_waits_for_a_key_the_cli_has_not_acted_on() {
    chat("hokeyq-me", Some(70));
    let (dir, file) = scratch_file("keyq");
    schedule("hokeyq-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    store::change("hokeyq-me", |r| r.switch.as_mut().unwrap().opened = Some("new".into())).unwrap();
    finish("hokeyq-me", 1, Ended::Failed { why: "x".into(), reset: true }, &|| "PROMPT".into());
    let line = reg().lock().unwrap().news.iter().find(|(c, _)| c == "hokeyq-me").map(|(_, l)| l.clone()).unwrap();
    set_conversation_for_test("hokeyq-me", Some("new"));
    crate::pty::fake_terminal_for_test("hokeyq-me", true);
    let told = as_told("hokeyq-me", &line);
    assert_eq!(told, line);
    // Their keys queued, the conversation not changed yet.
    let keys = Box::new(|| crate::pty::fake_person_keys("hokeyq-me"));
    crate::agents::notifier::deliver_news_for_test("hokeyq-me", vec![(told, line.clone())], keys);
    assert_eq!(crate::pty::fake_terminal_of("hokeyq-me"), (String::new(), false), "nothing typed, their keys go on");
    assert!(reg().lock().unwrap().news.iter().any(|(c, l)| c == "hokeyq-me" && *l == line), "still queued, for the next look");
    crate::pty::fake_terminal_for_test("hokeyq-me", false);
    reg().lock().unwrap().news.retain(|(c, _)| c != "hokeyq-me");
    let _ = store::remove("hokeyq-me");
    done_with(&["hokeyq-me"], &dir);
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
    let at = store::records()["homan-me"].asked.as_ref().unwrap().at;
    store::change("homan-me", |r| assert!(manual_typed(r, at, at + 1, "old".into()))).unwrap();
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
    // Asked again: its request typed only at a strict prompt, never through
    // the notifier, then its time runs out.
    open_manual("homan-me").unwrap();
    let at = store::records()["homan-me"].asked.as_ref().unwrap().at;
    manual_ask_of("homan-me", at + 1);
    assert!(news("homan-me").is_empty(), "nothing for the notifier to type");
    assert_eq!(store::records()["homan-me"].asked.as_ref().unwrap().typed, None, "no CLI at its prompt here: not typed");
    store::change("homan-me", |r| assert!(manual_typed(r, at, at + 2, "conv".into()))).unwrap();
    let typed = store::records()["homan-me"].asked.clone().unwrap();
    assert_eq!((typed.typed, typed.conversation.as_deref()), (Some(at + 2), Some("conv")), "gone in, into that conversation");
    store::change("homan-me", |r| assert!(!manual_typed(r, at, at + 3, "conv".into()), "once")).unwrap();
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
    // A move under way, asking the chat or not: quiet for the whole move.
    for phase in [
        moving::MovePhase::Asking { at: 1 },
        moving::MovePhase::Shown { at: 1 },
        moving::MovePhase::Confirmed { at: 1 },
        moving::MovePhase::RowCreated { at: 1 },
    ] {
        let m = moving::tests::a_move(phase);
        store::change("honow-me", |r| r.moving = Some(m)).unwrap();
        assert!(refused("honow-me").contains("move of this chat to another account is under way"), "{}", refused("honow-me"));
        assert!(open_manual("honow-me").is_err(), "looked at again where it is opened");
    }
    store::change("honow-me", |r| r.moving = Some(moving::tests::a_move(moving::MovePhase::Done { at: 2 }))).unwrap();
    assert_eq!(now_refusal("honow-me"), None, "a move over is no bar");
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
    // The conversation before loaded a skill, as Claude Code 2.1.322 records
    // its call and the result that says it did.
    let (dir, _) = scratch_file("gf");
    let previous = dir.join("old.jsonl");
    let call = json!({ "type": "assistant", "message": { "content": [
        { "type": "tool_use", "id": "toolu_1", "name": "Skill", "input": { "skill": "release" }, "caller": { "type": "direct" } }] } });
    let result = json!({ "type": "user", "message": { "role": "user", "content": [
        { "type": "tool_result", "tool_use_id": "toolu_1", "content": "Launching skill: release" }] } });
    // Then the threshold's notice, typed with a helper's line before it, whose
    // turn loads a skill to write the handoff: that one is left out.
    let tag = crate::agents::notifier::NOTICE_TAG;
    let asked = json!({ "type": "user", "message": { "role": "user",
        "content": format!("{tag} Your helper h1 answered. {}", facts::notice(71, 70, false, true)) } });
    let for_the_ask = json!({ "type": "assistant", "message": { "content": [
        { "type": "tool_use", "id": "toolu_2", "name": "Skill", "input": { "skill": "session-handoff" } }] } });
    let its_result = json!({ "type": "user", "message": { "role": "user", "content": [
        { "type": "tool_result", "tool_use_id": "toolu_2", "content": "Launching skill: session-handoff" }] } });
    std::fs::write(&previous, format!("{call}\n{result}\n{asked}\n{for_the_ask}\n{its_result}\n")).unwrap();
    let f = gather("hogf-me", Some(&previous.to_string_lossy()), Provider::Claude);
    assert_eq!(f.helpers.len(), 1);
    assert_eq!((f.helpers[0].id.as_str(), f.helpers[0].owes, f.helpers[0].account.as_str()), ("hogf-kid", true, "codex/two"));
    assert_eq!((f.helpers[0].model.as_deref(), f.helpers[0].effort.as_deref()), (Some("gpt-big"), Some("high")));
    assert_eq!(f.threshold, Some(70));
    assert!(f.owed_by_others.is_empty());
    assert_eq!(f.skills.map(|s| s.names), Some(vec!["release".to_string()]));
    // Where the conversation before is not known, or not to be read: not known.
    assert_eq!(gather("hogf-me", None, Provider::Claude).skills, None);
    assert_eq!(gather("hogf-me", Some(&dir.join("gone.jsonl").to_string_lossy()), Provider::Claude).skills, None);
    forget_session("hogf-kid");
    forget_session("hogf-me");
    let _ = std::fs::remove_dir_all(dir);
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
    set_conversation_for_test("hodr-ro", Some("old"));
    ask("hodr-ro", store::AskKind::Manual);
    let (failed, how) = call("hodr-ro", json!({}));
    assert!(!failed && how.contains("Codex's read-only sandbox, which writes no file"), "{how}");
    finish("hodr-me", 1, Ended::Done, &String::new);
    for id in ["hodr-me", "hodr-other", "hodr-ro"] {
        forget_session(id);
        set_conversation_for_test(id, None);
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
    let told = |level| store::Told { conversation: "c".into(), level, floor: 0, shares: vec![91], compacted: 0, tokens: 182_000 };
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
    assert_eq!(missing, ["Goal", "Next action", "The user's words", "State", "Pending", "Decisions and dead ends", "Skills to load first"]);
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
    let told = store::Told { conversation: "c".into(), level: 2, floor: 0, shares: vec![71, 86], compacted: 1, tokens: 172_000 };
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
    assert!(store::records()["hoask-me"].told.as_ref().is_some_and(|t| t.shares.is_empty() && t.compacted == 0));
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

/// The threshold's ask is opened only if what its look found still stands
/// under the record's lock: auto-handoff turned off or set elsewhere while the
/// context was read, the person's ask opened, or a handoff scheduled, and it
/// is not, nor is the chat told anything.
#[test]
fn the_thresholds_ask_is_not_opened_over_what_changed_as_its_context_was_read() {
    chat("hoaa-me", None);
    let (dir, file) = scratch_file("aa");
    let told = store::Told { conversation: "c".into(), level: 1, floor: 0, shares: vec![72], compacted: 0, tokens: 1 };
    let asked = store::Asked { conversation: Some("conv".into()), ..store::Asked::new(store::AskKind::Auto, 5, Some(1)) };
    let line = facts::notice(72, 70, false, true);
    let open = |threshold| {
        store::change("hoaa-me", |r| ask_auto("hoaa-me", r, threshold, told.clone(), false, asked.clone(), line.clone())).unwrap()
    };
    let queued = || reg().lock().unwrap().news.iter().filter(|(c, _)| c == "hoaa-me").count();
    // Off as it was read: its closing stands.
    set_auto_handoff("hoaa-me".into(), Some(70)).unwrap();
    set_auto_handoff("hoaa-me".into(), None).unwrap();
    assert!(!open(70));
    assert_eq!((open_ask("hoaa-me"), store::records()["hoaa-me"].told.clone()), (None, None), "nothing opened, nothing told");
    // Set at another threshold: the reading was for the old one.
    set_auto_handoff("hoaa-me".into(), Some(80)).unwrap();
    assert!(!open(70));
    set_auto_handoff("hoaa-me".into(), Some(70)).unwrap();
    // The person asked meanwhile: theirs stays.
    ask("hoaa-me", store::AskKind::Manual);
    assert!(!open(70));
    assert_eq!(open_ask("hoaa-me"), Some(store::AskKind::Manual));
    // A handoff scheduled meanwhile.
    schedule("hoaa-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    store::change("hoaa-me", |r| r.asked = None).unwrap();
    assert!(!open(70));
    finish("hoaa-me", 1, Ended::Done, &String::new);
    reg().lock().unwrap().news.retain(|(c, _)| c != "hoaa-me");
    // Nothing changed: opened, of the conversation read, its notice queued
    // with it under the record's lock.
    assert!(open(70));
    let notice = Some(sha256_of(&line));
    assert_eq!(store::records()["hoaa-me"].asked, Some(store::Asked { notice, ..asked.clone() }), "bound to its notice");
    assert_eq!(store::records()["hoaa-me"].told.as_ref().map(|t| t.level), Some(1));
    assert_eq!(queued(), 1);
    // The person's ask opened after it drops that notice, under the same lock.
    open_manual("hoaa-me").unwrap();
    assert_eq!(queued(), 0, "no second request");
    reg().lock().unwrap().news.retain(|(c, _)| c != "hoaa-me");
    done_with(&["hoaa-me"], &dir);
}

/// An ask holds only in the conversation it asks, and is closed once the
/// chat is found on another the person started, never while Luna's own
/// switch is under way. A call from another conversation, or from one Luna
/// cannot name, schedules nothing. The threshold's ask closes as the
/// threshold re-arms.
#[test]
fn an_ask_holds_only_in_the_conversation_it_asks() {
    chat("hoconv-me", Some(70));
    let (dir, file) = scratch_file("conv");
    let asked = || store::records()["hoconv-me"].asked.clone();
    let bound = store::Asked { conversation: Some("c1".into()), ..store::Asked::new(store::AskKind::Auto, 3, None) };
    store::change("hoconv-me", |r| r.asked = Some(bound.clone())).unwrap();
    // The chat's conversation not known: refused, not accepted.
    set_conversation_for_test("hoconv-me", None);
    let (failed, e) = call("hoconv-me", json!({}));
    assert!(failed && e.contains("cannot tell yet"), "{e}");
    set_conversation_for_test("hoconv-me", Some("old"));
    assert!(ask_holds(&bound, Some("c1")) && !ask_holds(&bound, Some("c2")) && !ask_holds(&bound, None));
    assert!(!drifted_to("hoconv-me", &bound, "c1"), "still there: still open");
    store::change("hoconv-me", |r| r.asked = Some(bound.clone())).unwrap();
    // A call made from another conversation than the one asked: nothing switches.
    let e = schedule("hoconv-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap_err();
    assert!(e.contains("another conversation") && e.contains("nothing will switch"), "{e}");
    let unknown = Before { conversation: None, ..before() };
    let e = schedule("hoconv-me", Provider::Claude, &file.to_string_lossy(), unknown).unwrap_err();
    assert!(e.contains("nothing will switch"), "a conversation not known: {e}");
    assert!(!holds_text("hoconv-me"));
    // The person's /clear: closed.
    assert!(drifted_to("hoconv-me", &bound, "c2"));
    assert_eq!(open_ask("hoconv-me"), None);
    let (failed, e) = call("hoconv-me", json!({}));
    assert!(failed && e.contains("nobody asked"), "{e}");
    // Luna's own switch moves the chat on: its ask is the switch's to close.
    store::change("hoconv-me", |r| {
        r.asked = Some(store::Asked { conversation: Some("old".into()), ..store::Asked::new(store::AskKind::Auto, 7, None) })
    })
    .unwrap();
    let n = schedule("hoconv-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap()["n"].as_u64().unwrap();
    assert!(!drifted_to("hoconv-me", &asked().unwrap(), "new"));
    assert!(asked().is_some(), "kept while the switch runs");
    finish("hoconv-me", n as u32, Ended::Done, &String::new);
    assert_eq!(asked(), None, "answered");
    // The threshold re-armed (a compaction, a larger window, another
    // conversation): its ask closes with its notice; the person's stays.
    ask("hoconv-me", store::AskKind::Manual);
    let at = |level| store::Told { conversation: "c3".into(), level, floor: 0, shares: vec![72], compacted: 0, tokens: 1 };
    store::change("hoconv-me", |r| {
        r.told = Some(at(1));
        assert!(!kept_told(r, at(0)), "the person's");
        assert!(r.asked.is_some());
        r.asked = Some(store::Asked::new(store::AskKind::Auto, 9, None));
        r.told = Some(at(1));
        assert!(!kept_told(r, at(1)) && r.asked.is_some(), "not re-armed");
        assert!(kept_told(r, at(0)) && r.asked.is_none(), "re-armed");
        r.asked = Some(store::Asked::new(store::AskKind::Auto, 10, None));
        assert!(!kept_told(r, at(0)) && r.asked.is_some(), "told nothing before");
        r.asked = None;
    })
    .unwrap();
    reg().lock().unwrap().news.retain(|(c, _)| c != "hoconv-me");
    done_with(&["hoconv-me"], &dir);
}

/// The person's ask authorizes nothing until its request is typed, and then
/// only in the conversation it went into: while Luna waits to type it, the
/// person may start another conversation, and text injected there must not
/// find the tool open. Neither bound nor closed while it waits.
#[test]
fn the_persons_ask_answers_only_once_typed_and_only_where_it_went() {
    chat("hopend-me", None);
    let (dir, file) = scratch_file("pend");
    open_manual("hopend-me").unwrap();
    let pending = store::records()["hopend-me"].asked.clone().unwrap();
    assert_eq!((pending.typed, pending.conversation.as_deref()), (None, None));
    assert!(!ask_holds(&pending, Some("old")) && !ask_holds(&pending, None));
    // The person's /new while it waits: no request was typed there.
    set_conversation_for_test("hopend-me", Some("theirs"));
    for args in [json!({}), json!({ "file": file.to_string_lossy() })] {
        let (failed, e) = call("hopend-me", args.clone());
        assert!(failed && e.contains("nobody asked"), "{args}: {e}");
    }
    let theirs = Before { conversation: Some("theirs".into()), ..before() };
    let e = schedule("hopend-me", Provider::Claude, &file.to_string_lossy(), theirs).unwrap_err();
    assert!(e.contains("nothing will switch"), "{e}");
    assert!(!holds_text("hopend-me"));
    assert!(!drifted_to("hopend-me", &pending, "theirs"));
    assert_eq!(store::records()["hopend-me"].asked, Some(pending.clone()), "neither bound nor closed");
    assert_eq!(handoff_ask_of("hopend-me").map(|a| (a.kind, a.typed)), Some(("manual", None)), "the pane: pending");
    // Typed into "theirs": answered there, and only there.
    store::change("hopend-me", |r| assert!(manual_typed(r, pending.at, pending.at + 1, "theirs".into()))).unwrap();
    let (failed, how) = call("hopend-me", json!({}));
    assert!(!failed && how.contains("How to hand this chat off"), "{how}");
    set_conversation_for_test("hopend-me", Some("later"));
    let (failed, e) = call("hopend-me", json!({}));
    assert!(failed && e.contains("nobody asked"), "{e}");
    // What the pane is told: typed, with its lapse.
    let typed = pending.at + 1;
    assert_eq!(
        handoff_ask_of("hopend-me").map(|a| (a.kind, a.typed, a.lapses_at)),
        Some(("manual", Some(typed), Some(typed + MANUAL_ASK_MS)))
    );
    store::change("hopend-me", |r| r.asked = None).unwrap();
    assert_eq!(handoff_ask_of("hopend-me"), None);
    done_with(&["hopend-me"], &dir);
}

/// A person's /clear while a scheduled handoff waits for rest: the switch is
/// given up, nothing reset, its copy kept, the ask closed, and the
/// conversation the person went on in hears what became of it.
#[test]
fn a_handoff_whose_chat_went_on_elsewhere_before_its_reset_is_given_up() {
    chat("holeft-me", Some(70));
    let (dir, file) = scratch_file("left");
    let n = schedule("holeft-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap()["n"].as_u64().unwrap() as u32;
    let s = store::records()["holeft-me"].switch.clone().unwrap();
    let theirs = Look { conversation: Some("theirs".into()), ..phase::tests::rest(s.scheduled_at + 10) };
    assert_eq!(phase::next(&s, &theirs, 5), Step::Left);
    finish("holeft-me", n, Ended::Left, &|| "PROMPT".into());
    assert!(!holds_text("holeft-me") && open_ask("holeft-me").is_none(), "released, and the ask with it");
    let rec = store::records()["holeft-me"].clone();
    let past = rec.past.last().unwrap();
    assert!(!past.done && past.why.as_deref() == Some(phase::LEFT), "{past:?}");
    assert!(Path::new(&past.snapshot).exists(), "the copy is kept");
    let news: Vec<String> = reg().lock().unwrap().news.iter().filter(|(c, _)| c == "holeft-me").map(|(_, l)| l.clone()).collect();
    assert_eq!(news, [facts::left_line(n, &past.snapshot)]);
    assert!(!news[0].contains("PROMPT") && news[0].contains("cleared nothing"), "{}", news[0]);
    reg().lock().unwrap().news.retain(|(c, _)| c != "holeft-me");
    done_with(&["holeft-me"], &dir);
}

/// The person's ask lapses only as the one looked at, and never under a
/// handoff a call scheduled since that look; nor is it taken as typed then.
#[test]
fn the_persons_ask_does_not_lapse_under_a_handoff_scheduled_since_the_look() {
    chat("holap-me", None);
    let (dir, file) = scratch_file("lap");
    open_manual("holap-me").unwrap();
    let at = store::records()["holap-me"].asked.as_ref().unwrap().at;
    store::change("holap-me", |r| assert!(manual_typed(r, at, at + 1, "old".into()))).unwrap();
    let looked = store::records()["holap-me"].asked.clone().unwrap();
    // A call schedules its handoff between the look and the lapse.
    schedule("holap-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    assert!(!store::change("holap-me", |r| lapsed(r, &looked)).unwrap());
    assert_eq!(store::records()["holap-me"].asked, Some(looked.clone()), "the ask it answers stays");
    store::change("holap-me", |r| assert!(!manual_typed(r, at, at + 2, "old".into()), "nor typed under it")).unwrap();
    finish("holap-me", 1, Ended::Done, &String::new);
    // Another ask in its place since the look: not that one.
    ask("holap-me", store::AskKind::Manual);
    assert!(!store::change("holap-me", |r| lapsed(r, &looked)).unwrap());
    assert_eq!(open_ask("holap-me"), Some(store::AskKind::Manual));
    let now = store::records()["holap-me"].asked.clone().unwrap();
    assert!(store::change("holap-me", |r| lapsed(r, &now)).unwrap());
    assert_eq!(open_ask("holap-me"), None);
    reg().lock().unwrap().news.retain(|(c, _)| c != "holap-me");
    done_with(&["holap-me"], &dir);
}

/// What a chat is found on is no evidence of its conversation unless the
/// transcript is that conversation's own: after Codex's `/new` the rollout is
/// the old thread's until the new one writes its own, and after a `/clear` a
/// transcript not found now is the last one found. The threshold opens no
/// ask on it.
#[test]
fn a_transcript_is_the_conversation_s_only_when_it_is_its_own() {
    let (a, b) = ("019a0000-0000-7000-8000-00000000000a", "019a0000-0000-7000-8000-00000000000b");
    let rollout = |id: &str| PathBuf::from(format!("/s/2026/11/12/rollout-2026-11-12T10-00-00-{id}.jsonl"));
    assert!(transcript_is(&rollout(a), Provider::Codex, a));
    assert!(transcript_is(&rollout(a), Provider::Codex, &a[..25]), "the start of it, as Codex's title names it");
    assert!(!transcript_is(&rollout(a), Provider::Codex, b), "after /new: the thread before's rollout");
    assert!(!transcript_is(&rollout(a), Provider::Codex, ""));
    let claude = |id: &str| PathBuf::from(format!("/p/{id}.jsonl"));
    assert!(transcript_is(&claude("s1"), Provider::Claude, "s1"));
    assert!(!transcript_is(&claude("s1"), Provider::Claude, "s2"), "after /clear: the transcript found last");
}

/// The person's ask whose request Luna could never type (the chat never at
/// its prompt, its conversation not known, its CLI not running) is withdrawn
/// in time, from when it was opened, with its own word to the chat; only
/// that same ask, never one typed since; and the person may ask again.
#[test]
fn the_persons_ask_never_typed_is_withdrawn_in_time_saying_so() {
    chat("hound-me", None);
    open_manual("hound-me").unwrap();
    let a = store::records()["hound-me"].asked.clone().unwrap();
    assert!(now_refusal("hound-me").is_some_and(|e| e.contains("asked for a handoff already") && e.contains("withdrawn")));
    assert!(now_refusal("hound-me").is_some_and(|e| !e.contains("  ")), "no run of spaces from a lost line break");
    manual_ask_of("hound-me", a.at + MANUAL_DELIVER_MS);
    assert_eq!(open_ask("hound-me"), Some(store::AskKind::Manual), "not before its time");
    // A core started later goes by when it was opened, not by its own start.
    let back = store::from_bytes(&serde_json::to_vec(&*store::records()).unwrap(), a.at + MANUAL_DELIVER_MS);
    assert_eq!(back["hound-me"].asked.as_ref().map(|x| x.at), Some(a.at));
    manual_ask_of("hound-me", a.at + MANUAL_DELIVER_MS + 1);
    assert_eq!(open_ask("hound-me"), None, "withdrawn");
    let news: Vec<String> = reg().lock().unwrap().news.iter().filter(|(c, _)| c == "hound-me").map(|(_, l)| l.clone()).collect();
    assert_eq!(news, [facts::manual_undelivered(MANUAL_DELIVER_MS / 60_000)]);
    assert_ne!(news[0], facts::manual_lapsed(MANUAL_ASK_MS / 60_000), "not the typed request's lapse");
    for line in [&news[0], &facts::manual_lapsed(10), &facts::manual_notice(), NOT_YET, NOT_ASKED] {
        assert!(!line.contains("  "), "{line}");
    }
    assert!(now_refusal("hound-me").is_none(), "asked again freely");
    // Typed since the look: that ask stays.
    open_manual("hound-me").unwrap();
    let looked = store::records()["hound-me"].asked.clone().unwrap();
    store::change("hound-me", |r| assert!(manual_typed(r, looked.at, looked.at + 1, "old".into()))).unwrap();
    undelivered("hound-me", &looked);
    assert_eq!(store::records()["hound-me"].asked.as_ref().and_then(|x| x.typed), Some(looked.at + 1), "typed: not withdrawn");
    reg().lock().unwrap().news.retain(|(c, _)| c != "hound-me");
    store::change("hound-me", |r| r.asked = None).unwrap();
    forget_session("hound-me");
    set_conversation_for_test("hound-me", None);
}

/// The person's ask that took the threshold's place, closed with no handoff
/// (lapsed or withdrawn), leaves the threshold armed: the conversation past
/// it is asked again at its next turn's end, not left unasked at the level
/// it was told of. So does auto-handoff turned off and on again.
#[test]
fn the_threshold_asks_again_once_what_took_its_place_closes() {
    chat("horearm-me", Some(70));
    let told = store::Told { conversation: "c".into(), level: 1, floor: 0, shares: vec![72], compacted: 0, tokens: 1 };
    store::change("horearm-me", |r| r.told = Some(told.clone())).unwrap();
    open_manual("horearm-me").unwrap();
    let level = || store::records()["horearm-me"].told.as_ref().map(|t| t.level);
    assert_eq!(level(), Some(1), "the person's ask open: nothing re-armed");
    let a = store::records()["horearm-me"].asked.clone().unwrap();
    assert!(store::change("horearm-me", |r| lapsed(r, &a)).unwrap());
    let after = store::records()["horearm-me"].told.clone().unwrap();
    assert_eq!((after.level, after.shares.clone()), (0, vec![72]), "re-armed, what it was asked at kept");
    assert!(watch::due(&after, watch::level(75, 70, after.floor)), "asked again past the threshold");
    // Off and on again at the same threshold.
    store::change("horearm-me", |r| r.told = Some(told.clone())).unwrap();
    set_auto_handoff("horearm-me".into(), None).unwrap();
    assert_eq!(level(), Some(1));
    set_auto_handoff("horearm-me".into(), Some(70)).unwrap();
    assert_eq!(level(), Some(0), "on again: armed");
    // On with the threshold's ask open: it stands, and so does its level.
    store::change("horearm-me", |r| r.told = Some(told.clone())).unwrap();
    ask("horearm-me", store::AskKind::Auto);
    set_auto_handoff("horearm-me".into(), Some(70)).unwrap();
    assert_eq!(level(), Some(1));
    store::change("horearm-me", |r| r.asked = None).unwrap();
    reg().lock().unwrap().news.retain(|(c, _)| c != "horearm-me");
    forget_session("horearm-me");
    set_conversation_for_test("horearm-me", None);
}

/// `turns_ended` turns over, the chat at rest (`Idle`) or not.
fn turn(idle: bool, turns_ended: u32) -> Option<crate::activity::TurnState> {
    let turn = if idle { crate::activity::Turn::Idle } else { crate::activity::Turn::Busy };
    Some(crate::activity::TurnState { turn, busy: !idle, turns_ended, children_busy: false })
}

/// A look's reading of conversation `live` at `pct` of its context.
fn reading(live: &str, pct: u64) -> Reading {
    Reading { live: live.into(), conversation: live.into(), pct, tokens: pct * 2_000, floor_at: None }
}

/// A conversation a handoff began keeps as its floor where its first turn
/// ended, however late it is first looked at: at that turn's end; after a
/// notice from a helper, or a draft the person sent, went in before the
/// beat's look; after auto-handoff turned on later. One that began above the
/// threshold is not asked again for that alone; one grown past it since is.
/// A conversation the person opened has no floor.
#[test]
fn a_conversation_a_handoff_began_keeps_the_floor_its_first_turn_ended_at() {
    chat("hofloor-me", None);
    listed("hofloor-me");
    let fresh = || store::change("hofloor-me", |r| (r.fresh, r.told, r.asked) = (true, None, None)).unwrap();
    let at = |pct, floor_at| Reading { floor_at, ..reading("next", pct) };
    let asked = || store::records()["hofloor-me"].asked.as_ref().map(|a| a.kind);
    let floor = || store::records()["hofloor-me"].told.as_ref().map(|t| t.floor);
    set_auto_handoff("hofloor-me".into(), Some(10)).unwrap();
    // Looked at as its first turn ends, at 12% under a threshold of 10.
    fresh();
    look_at("hofloor-me", 10, &at(12, Some(12)), 3, check_due("hofloor-me"));
    assert_eq!((asked(), floor()), (None, Some(12)));
    // A notice, or the person's draft sent, went in first: looked at at 14%,
    // its first turn having ended at 12%. Not asked; at 17% it is.
    fresh();
    look_at("hofloor-me", 10, &at(14, Some(12)), 4, None);
    assert_eq!((asked(), floor()), (None, Some(12)), "the floor is where its first turn ended");
    look_at("hofloor-me", 10, &at(17, Some(12)), 5, None);
    assert_eq!(asked(), Some(store::AskKind::Auto), "grown past it");
    reg().lock().unwrap().news.retain(|(c, _)| c != "hofloor-me");
    // Handed off by the person with auto-handoff off, its first turn ended at
    // 20%, turns of its own since: auto-handoff turned on at 50% with the
    // chat at 60% asks at once.
    fresh();
    set_auto_handoff("hofloor-me".into(), Some(50)).unwrap();
    look_at("hofloor-me", 50, &at(60, Some(20)), 9, check_due("hofloor-me"));
    assert_eq!((asked(), floor()), (Some(store::AskKind::Auto), Some(20)));
    assert_eq!(queued("hofloor-me"), [facts::notice(60, 50, false, true)]);
    reg().lock().unwrap().news.retain(|(c, _)| c != "hofloor-me");
    // A conversation Luna's prompt did not open: no floor.
    fresh();
    set_auto_handoff("hofloor-me".into(), Some(10)).unwrap();
    look_at("hofloor-me", 10, &at(12, None), 9, check_due("hofloor-me"));
    assert_eq!((asked(), floor()), (Some(store::AskKind::Auto), Some(0)));
    reg().lock().unwrap().news.retain(|(c, _)| c != "hofloor-me");
    let _ = store::remove("hofloor-me");
    forget_session("hofloor-me");
    set_conversation_for_test("hofloor-me", None);
}

/// Auto-handoff turned off between the notifier putting a threshold notice
/// together and typing it: the ask it came with is closed, so it is not
/// typed, nor is it once the ask is another (a later notice's), auto-handoff
/// is set again meanwhile, or the chat is on another conversation than the
/// one asked. Typed only while all of it stands.
#[test]
fn a_threshold_notice_put_together_goes_in_only_while_its_ask_stands() {
    chat("hooffq-me", None);
    listed("hooffq-me");
    crate::pty::fake_terminal_for_test("hooffq-me", true);
    set_auto_handoff("hooffq-me".into(), Some(70)).unwrap();
    let ask = |pct| {
        store::change("hooffq-me", |r| (r.told, r.asked) = (None, None)).unwrap();
        look_at("hooffq-me", 70, &reading("old", pct), 2, None);
        queued("hooffq-me").pop().unwrap()
    };
    let deliver = |line: &str, between: Box<dyn FnOnce() + Send>| {
        crate::agents::notifier::deliver_news_for_test("hooffq-me", vec![(line.to_string(), line.to_string())], between);
        let (typed, _) = crate::pty::fake_terminal_of("hooffq-me");
        crate::pty::fake_terminal_for_test("hooffq-me", true);
        !typed.is_empty()
    };
    // Turned off between: nothing typed, and nothing left to type.
    let line = ask(75);
    let off = Box::new(|| {
        set_auto_handoff("hooffq-me".into(), None).unwrap();
    });
    assert!(!deliver(&line, off), "no ask for it to answer");
    assert!(withheld("hooffq-me", &line) && queued("hooffq-me").is_empty());
    // Replaced by a later ask's notice: the earlier copy is not typed.
    set_auto_handoff("hooffq-me".into(), Some(70)).unwrap();
    let first = ask(75);
    store::change("hooffq-me", |r| r.asked.as_mut().unwrap().notice = Some("another".into())).unwrap();
    assert!(withheld("hooffq-me", &first), "not typed, and dropped at the notifier's next look");
    reg().lock().unwrap().news.retain(|(c, _)| c != "hooffq-me");
    // On another conversation than the one asked: not now, still queued for
    // the beat to close the ask.
    let line = ask(76);
    assert!(!deliver(&line, Box::new(|| set_conversation_for_test("hooffq-me", Some("theirs")))));
    assert!(!withheld("hooffq-me", &line) && queued("hooffq-me") == [line.clone()]);
    // Back on it, nothing changed: what the notifier looks at under the held
    // keys lets it go in.
    set_conversation_for_test("hooffq-me", Some("old"));
    assert!(notice_here("hooffq-me", &line) && !withheld("hooffq-me", &line));
    assert!(notice_here("hooffq-me", "Your helper h1 answered"), "any other line, as far as this goes");
    crate::pty::fake_terminal_for_test("hooffq-me", false);
    reg().lock().unwrap().news.retain(|(c, _)| c != "hooffq-me");
    let _ = store::remove("hooffq-me");
    forget_session("hooffq-me");
    set_conversation_for_test("hooffq-me", None);
}

/// A threshold notice whose chat Luna cannot place on the conversation it
/// asks (not known now) is left out of the notice the notifier puts
/// together, still queued: what else that notice says, a helper's answer,
/// goes without it. Placed again, it is taken.
#[test]
fn a_threshold_notice_that_cannot_be_placed_holds_up_nothing_else() {
    chat("hoplace-me", None);
    listed("hoplace-me");
    set_auto_handoff("hoplace-me".into(), Some(70)).unwrap();
    look_at("hoplace-me", 70, &reading("old", 75), 2, None);
    let notice = queued("hoplace-me").pop().unwrap();
    let answer = "Your helper h1 answered.".to_string();
    reg().lock().unwrap().news.push(("hoplace-me".into(), answer.clone()));
    let notice_of = |news: &[String]| HashMap::from([("hoplace-me".to_string(), news.to_vec())]);
    set_conversation_for_test("hoplace-me", None);
    let mut news = notice_of(&[notice.clone(), answer.clone()]);
    crate::agents::notifier::placed_now(&mut news);
    assert_eq!(news["hoplace-me"], std::slice::from_ref(&answer), "the answer goes without it");
    assert!(queued("hoplace-me").contains(&notice), "still queued, for a later notice");
    // Only a threshold notice: nothing at all left for this look.
    let mut alone = notice_of(std::slice::from_ref(&notice));
    crate::agents::notifier::placed_now(&mut alone);
    assert!(alone.is_empty());
    // Placed again.
    set_conversation_for_test("hoplace-me", Some("old"));
    let mut news = notice_of(&[notice.clone(), answer.clone()]);
    crate::agents::notifier::placed_now(&mut news);
    assert_eq!(news["hoplace-me"], [notice, answer]);
    reg().lock().unwrap().news.retain(|(c, _)| c != "hoplace-me");
    let _ = store::remove("hoplace-me");
    forget_session("hoplace-me");
    set_conversation_for_test("hoplace-me", None);
}

/// The person's `/new` sent just before the notifier held their keys has not
/// reached the CLI yet, which still reads as the conversation the threshold
/// asked: a key of theirs since the notifier's look keeps the notice back,
/// still queued, as it does a continuation, without the notice being taken
/// for one.
#[test]
fn a_threshold_notice_waits_for_a_key_the_cli_has_not_acted_on() {
    chat("hokeyt-me", None);
    listed("hokeyt-me");
    set_auto_handoff("hokeyt-me".into(), Some(70)).unwrap();
    look_at("hokeyt-me", 70, &reading("old", 75), 2, None);
    let notice = queued("hokeyt-me").pop().unwrap();
    assert!(keyed_to_conversation(&notice) && !bound_to_conversation(&notice));
    crate::pty::fake_terminal_for_test("hokeyt-me", true);
    let keys = Box::new(|| crate::pty::fake_person_keys("hokeyt-me"));
    crate::agents::notifier::deliver_news_for_test("hokeyt-me", vec![(notice.clone(), notice.clone())], keys);
    assert_eq!(crate::pty::fake_terminal_of("hokeyt-me"), (String::new(), false), "nothing typed, their keys go on");
    assert!(queued("hokeyt-me").contains(&notice), "still queued, for the next look");
    crate::pty::fake_terminal_for_test("hokeyt-me", false);
    reg().lock().unwrap().news.retain(|(c, _)| c != "hokeyt-me");
    let _ = store::remove("hokeyt-me");
    forget_session("hokeyt-me");
    set_conversation_for_test("hokeyt-me", None);
}

fn queued(id: &str) -> Vec<String> {
    reg().lock().unwrap().news.iter().filter(|(c, _)| c == id).map(|(_, l)| l.clone()).collect()
}

/// Auto-handoff turned on with the chat already past the threshold: its
/// context is looked at as soon as it is at rest, at the turn count its last
/// turn's end was looked at, not left until its next turn ends; by the one
/// look a turn's end takes, the ask bound to the conversation read and its
/// notice queued for the notifier, which holds it behind a draft as any.
#[test]
fn turning_it_on_past_the_threshold_asks_as_soon_as_the_chat_is_at_rest() {
    chat("honow-me", None);
    listed("honow-me");
    let mut b = Beat::default();
    assert_eq!(to_look(&mut b, "honow-me", turn(true, 4)), Some((4, None)), "a turn's end not looked at yet");
    assert_eq!(to_look(&mut b, "honow-me", turn(true, 4)), None, "looked at: nothing until the next turn's end");
    set_auto_handoff("honow-me".into(), Some(70)).unwrap();
    let mark = check_due("honow-me");
    assert!(mark.is_some(), "on: to be looked at now");
    assert_eq!(to_look(&mut b, "honow-me", turn(false, 4)), None, "mid-turn: not at rest yet");
    assert_eq!(check_due("honow-me"), mark, "the mark waits");
    assert_eq!(to_look(&mut b, "honow-me", turn(true, 4)), Some((4, mark)), "at rest: looked at, at the same turn");
    look_at("honow-me", 70, &reading("conv-live", 75), 4, mark);
    assert_eq!(check_due("honow-me"), None, "its look over");
    let asked = store::records()["honow-me"].asked.clone().unwrap();
    assert_eq!((asked.kind, asked.conversation.as_deref(), asked.turns), (store::AskKind::Auto, Some("conv-live"), Some(4)));
    assert_eq!(queued("honow-me"), [facts::notice(75, 70, false, true)], "through the notifier, as at a turn's end");
    assert_eq!(to_look(&mut b, "honow-me", turn(true, 4)), None, "once");
    // On again at another threshold while its ask is open: looked at again,
    // and the conversation, told already, is not asked a second time.
    set_auto_handoff("honow-me".into(), Some(60)).unwrap();
    let again = check_due("honow-me");
    assert!(again.is_some() && again != mark);
    look_at("honow-me", 60, &reading("conv-live", 75), 4, mark);
    assert_eq!(check_due("honow-me"), again, "an old look lets go of its own mark only");
    look_at("honow-me", 60, &reading("conv-live", 75), 4, again);
    assert_eq!((check_due("honow-me"), queued("honow-me").len()), (None, 1));
    // Under its threshold: told nothing, the mark let go all the same.
    store::change("honow-me", |r| (r.asked, r.told) = (None, None)).unwrap();
    reg().lock().unwrap().news.retain(|(c, _)| c != "honow-me");
    set_auto_handoff("honow-me".into(), Some(80)).unwrap();
    let low = check_due("honow-me");
    look_at("honow-me", 80, &reading("conv-live", 75), 4, low);
    assert_eq!((check_due("honow-me"), open_ask("honow-me"), queued("honow-me").len()), (None, None, 0));
    forget_session("honow-me");
    set_conversation_for_test("honow-me", None);
}

/// The look turning auto-handoff on asks for waits while the chat cannot be
/// asked: its CLI has not listed the tool (the window's quick resume brings
/// it), a move or a handoff of it is under way, or the person's ask is open.
/// Turned off before the notice went in, the ask, the notice and the mark
/// go. A core started again in between looks at the chat at its first beat
/// all the same, its record re-armed as it loads.
#[test]
fn a_look_turning_it_on_asks_for_waits_until_the_chat_may_be_asked() {
    chat("hogate-me", None);
    let mut b = Beat::default();
    b.turns.insert("hogate-me".into(), 2);
    let look = |b: &mut Beat| to_look(b, "hogate-me", turn(true, 2));
    set_auto_handoff("hogate-me".into(), Some(70)).unwrap();
    let mark = check_due("hogate-me");
    assert_eq!(look(&mut b), None, "its CLI has not listed the tool");
    listed("hogate-me");
    let shown = moving::tests::a_move(moving::MovePhase::Shown { at: 1 });
    store::change("hogate-me", |r| r.moving = Some(shown)).unwrap();
    assert_eq!(look(&mut b), None, "a move under way keeps the threshold quiet");
    store::change("hogate-me", |r| r.moving = Some(moving::tests::a_move(moving::MovePhase::Canceled { at: 2 }))).unwrap();
    ask("hogate-me", store::AskKind::Manual);
    assert_eq!(look(&mut b), None, "the person's ask is the chat's request already");
    store::change("hogate-me", |r| r.asked = None).unwrap();
    let (dir, file) = scratch_file("gate");
    ask("hogate-me", store::AskKind::Auto);
    schedule("hogate-me", Provider::Claude, &file.to_string_lossy(), before()).unwrap();
    assert_eq!(look(&mut b), None, "a handoff under way");
    finish("hogate-me", 1, Ended::Done, &String::new);
    // Read here as the conversation before, not the one the handoff began,
    // which would keep its first reading as a floor.
    store::change("hogate-me", |r| r.fresh = false).unwrap();
    assert_eq!(check_due("hogate-me"), mark, "the mark waited through all of it");
    assert_eq!(look(&mut b), Some((2, mark)));
    look_at("hogate-me", 70, &reading("conv", 80), 2, mark);
    assert_eq!(queued("hogate-me").len(), 1);
    // Off before the notifier typed it: nothing of it is left to ask.
    set_auto_handoff("hogate-me".into(), Some(70)).unwrap();
    set_auto_handoff("hogate-me".into(), None).unwrap();
    assert_eq!((open_ask("hogate-me"), check_due("hogate-me"), queued("hogate-me").len()), (None, None, 0));
    let (failed, e) = call("hogate-me", json!({}));
    assert!(failed && e.contains("nobody asked"), "{e}");
    // On, and the core gone before the look, the mark with it: the record was
    // saved re-armed as auto-handoff went on, a new core takes it up so, and
    // its first beat, which has looked at no turn yet, looks at the chat at
    // rest all the same.
    set_auto_handoff("hogate-me".into(), Some(70)).unwrap();
    unmark("hogate-me", None);
    let saved = serde_json::to_vec(&*store::records()).unwrap();
    let back = store::from_bytes(&saved, now_ms());
    assert_eq!(back["hogate-me"].told.as_ref().map(|t| t.level), Some(0), "re-armed");
    assert_eq!(back["hogate-me"].asked, None);
    let mut fresh = Beat::default();
    assert_eq!(to_look(&mut fresh, "hogate-me", turn(true, 2)), Some((2, None)));
    look_at("hogate-me", 70, &reading("conv", 80), 2, None);
    assert_eq!(open_ask("hogate-me"), Some(store::AskKind::Auto), "asked again");
    reg().lock().unwrap().news.retain(|(c, _)| c != "hogate-me");
    let _ = store::remove("hogate-me");
    done_with(&["hogate-me"], &dir);
}
