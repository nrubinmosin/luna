//! What never reached a session: text held for it that it exited with, or a
//! first turn its CLI came back from a core's going without.

use crate::agents::ask::{live, state_of};
use crate::agents::delivery::Held;
use crate::agents::owed::hand_over;
use crate::agents::persist::{conversation_on_disk, Opening};
use crate::agents::read::MAX_TEXT;
use crate::agents::registry::{label, parent_of, reg};
use crate::agents::said::note_file;
use crate::agents::wait::{Waited, LET_GO};

/// Whether a visible helper has nothing to carry on from, as one
/// `background::never_started` says so of has not: its first turn went with a core,
/// and its CLI came back afresh, or has not come back and has no conversation on disk
/// to come back to. Not while it is yet to go on by itself, which gives it its prompt.
pub(super) fn first_turn_lost(pty: &crate::pty::PtyManager, id: &str) -> bool {
    let count = crate::activity::turns_counted(id).unwrap_or(0);
    let known = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let Some(o) = r.openings.get(id) else { return false };
        if r.resumes.contains_key(id) {
            return false;
        }
        if o.lost {
            return true;
        }
        if o.floor == 0 || count > o.floor {
            return false;
        }
        r.known.get(id).cloned()
    };
    if pty.alive(id) {
        return false;
    }
    known.is_none_or(|k| conversation_on_disk(id, k.provider, &k.folder, &k.account_path).is_none())
}

/// Whether what `caller` is to hear of `id` is all there will be: it was asked, a core
/// went, and nothing has that but Luna. No turn is coming to answer it, and what Luna
/// holds of the caller's stays for this rather than going in.
pub(super) fn lost_whole(pty: &crate::pty::PtyManager, caller: &str, id: &str) -> bool {
    parent_of(id).as_deref() == Some(caller) && (crate::background::never_started(id) || first_turn_lost(pty, id))
}

/// `lost_whole` of a visible helper with nothing asked of it since: `caller` is owed
/// the turn a core went with and no other. Asked since, a text that went in has started
/// it, and what it says to that is still to come, though what it was first asked is lost.
pub(super) fn still_all_lost(pty: &crate::pty::PtyManager, caller: &str, id: &str) -> bool {
    let cut = reg().lock().unwrap_or_else(|e| e.into_inner()).owed.get(&(caller.to_string(), id.to_string())).is_none_or(|o| o.cut_short);
    cut && parent_of(id).as_deref() == Some(caller) && first_turn_lost(pty, id)
}

/// The count that hands over a debt on a first turn lost: past a turn that went with a
/// core, which a session not back has no track to count. A question asked since is no
/// such debt, and still waits for its turn.
pub(super) fn lost_count(caller: &str, id: &str) -> u32 {
    let count = crate::activity::turn_state(id).map(|t| t.turns_ended).or_else(|| crate::activity::turns_counted(id)).unwrap_or(0);
    let cut = reg()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .owed
        .get(&(caller.to_string(), id.to_string()))
        .filter(|o| o.cut_short)
        .map(|o| o.baseline.saturating_add(1));
    count.max(cut.unwrap_or(0))
}

/// A `wait` on a helper whose first turn is lost: what there is, at once, handed over
/// as any answer is, so that a cancel crossing it puts it back with the debt.
pub(super) fn lost_answer(caller: &str, id: &str, call: &crate::mcp::Call, alone: bool) -> Result<(Waited, bool), String> {
    let turns_ended = lost_count(caller, id);
    let undelivered = undelivered_note(caller, id);
    if alone && !call.commit(hand_over(caller, id, turns_ended, None)) {
        return Err(LET_GO.into());
    }
    let outcome = if live(crate::pty::manager(), id) { "turn_done" } else { "exit" };
    let (state, ask) = state_of(id);
    let failed = Some("it never got started: Luna's core restarted before its first turn got anywhere".to_string());
    Ok((
        Waited {
            outcome,
            state,
            ask,
            turns_ended,
            reply: None,
            failed,
            transcript_missing: false,
            screen: None,
            undelivered,
            job_debt: None,
        },
        true,
    ))
}

/// What never reached `id` that `caller` is to hear of: what a background helper
/// keeps for it (`background::undelivered_for`), and for the parent of a visible one
/// that has lost its first turn, what it first asked and what Luna holds of its.
pub(super) fn undelivered_for(id: &str, caller: &str, parent: bool) -> Vec<String> {
    let mut lost = crate::background::undelivered_for(id, caller, parent);
    if parent && first_turn_lost(crate::pty::manager(), id) {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        lost.extend(r.openings.get(id).map(|o| o.text.clone()));
        lost.extend(r.held.iter().filter(|h| h.id == id && h.caller == caller).map(|h| h.text.clone()));
    }
    lost
}

/// What `take_undelivered` took off, for a cancel to put back.
pub(super) struct Undelivered {
    pub(super) queued: Vec<crate::background::Queued>,
    pub(super) opening: Option<Opening>,
    pub(super) held: Vec<Held>,
    worktrees: Option<Vec<String>>,
}

/// Takes off what `undelivered_for` gives, and the worktrees the parent hears of
/// with it, `caller` having been told.
pub(super) fn take_undelivered(id: &str, caller: &str, parent: bool) -> Undelivered {
    let queued = crate::background::take_undelivered_for(id, caller, parent);
    let lost = parent && first_turn_lost(crate::pty::manager(), id);
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    let opening = if lost { r.openings.remove(id) } else { None };
    let mut held = Vec::new();
    if lost {
        let (mine, rest): (Vec<Held>, Vec<Held>) = std::mem::take(&mut r.held).into_iter().partition(|h| h.id == id && h.caller == caller);
        r.held = rest;
        held = mine;
    }
    let worktrees = if parent { r.worktrees_left.remove(id) } else { None };
    Undelivered { queued, opening, held, worktrees }
}

pub(super) fn put_undelivered_back(id: &str, taken: Undelivered) {
    crate::background::put_back(id, taken.queued);
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(opening) = taken.opening {
        r.openings.insert(id.to_string(), opening);
    }
    let later = std::mem::replace(&mut r.held, taken.held);
    r.held.extend(later);
    if let Some(worktrees) = taken.worktrees {
        r.worktrees_left.insert(id.to_string(), worktrees);
    }
}

/// What never reached a helper (`undelivered_for`), as its answer file ends on
/// it: labeled so that it is not read as anything the helper said.
pub(super) fn undelivered_text(lost: &[String]) -> String {
    let mut text = format!(
        "NOT DELIVERED: {} meant for this helper when Luna's core stopped its turn or its CLI went. It never acted on them unless marked; send again what still matters. \
         Word for word, oldest first (the first is its opening prompt, if it never got started):",
        count_of(lost)
    );
    for (i, message) in lost.iter().enumerate() {
        text.push_str(&format!("\n\n--- message {} of {} ---\n\n{message}", i + 1, lost.len()));
    }
    text
}

/// Words that end a sentence in a notice, with a stop unless they bring their
/// own: a CLI's error mostly does ("…pick a different model.").
pub(super) fn sentence(words: &str) -> String {
    let words = words.trim_end();
    if words.ends_with(['.', '!', '?', '…']) {
        words.to_string()
    } else {
        format!("{words}.")
    }
}

fn count_of(messages: &[String]) -> String {
    match messages.len() {
        1 => "1 message".to_string(),
        n => format!("{n} messages"),
    }
}

/// The notice's words on what never reached the helper, from the file that
/// holds it; `after_answer` when the file opens on the answer instead.
pub(super) fn undelivered_line(lost: &[String], after_answer: bool, file: Option<&std::path::Path>) -> String {
    if lost.is_empty() {
        return String::new();
    }
    let what = count_of(lost);
    match file {
        Some(_) if after_answer => format!(" Not delivered: {what} queued for it, at the end of that file; send again what still matters."),
        Some(path) => format!(" Not delivered: {what} queued for it, in {}; send again what still matters.", path.display()),
        None => format!(" Not delivered: {what} queued for it; luna_read gives them, send again what still matters."),
    }
}

/// The same for a tool result: where they are, or them, cut as any words are
/// that one result carries, should no file take them. Only what `caller` itself
/// is to hear of.
pub(super) fn undelivered_note(caller: &str, id: &str) -> Option<String> {
    let parent = parent_of(id).as_deref() == Some(caller);
    let lost = undelivered_for(id, caller, parent);
    let left = if parent { worktrees_left_line(id) } else { String::new() };
    if lost.is_empty() {
        return (!left.is_empty()).then(|| left.trim_start().to_string());
    }
    let text = undelivered_text(&lost);
    let whole = lost_whole(crate::pty::manager(), caller, id);
    Some(match note_file(id, &text) {
        Some(path) if whole => {
            format!(
                "It never got started: Luna's core restarted before its first turn got anywhere; {}. Send again what still matters.{left}",
                never_started_what(&lost, Some(&path))
            )
        }
        Some(path) => format!("{}{left}", undelivered_line(&lost, false, Some(&path)).trim_start()),
        None => format!("{}{left}", text.chars().take(MAX_TEXT).collect::<String>()),
    })
}

/// How a notice says what it gives back of a helper that never got started.
pub(super) fn never_started_what(lost: &[String], file: Option<&std::path::Path>) -> String {
    match (lost.len(), file) {
        (_, None) => "its prompt was not kept".to_string(),
        (1, Some(path)) => format!("its prompt is in {}", path.display()),
        (n, Some(path)) => {
            format!("its prompt and {} are in {}", count_of(&lost[1.min(n)..]).replace("message", "queued message"), path.display())
        }
    }
}

/// What a notice says of worktrees begun and not gone on in. Not all are known for its
/// own (Codex's go by what is new), so it says what Luna can stand behind.
pub(super) fn left_words(left: &[String]) -> String {
    if left.is_empty() {
        return String::new();
    }
    format!(" worktreeLeft: {}: begun for it as far as Luna can tell; nothing runs there and Luna has not removed it.", left.join(", "))
}

/// The same of what a helper a core cut off left (`Registry::worktrees_left`).
pub(super) fn worktrees_left_line(id: &str) -> String {
    reg().lock().unwrap_or_else(|e| e.into_inner()).worktrees_left.get(id).map_or(String::new(), |left| left_words(left))
}

/// A helper leaves with text that never reached it and nobody told: somebody's
/// words all the same, so to a file the log names.
pub fn undelivered_left(id: &str, lost: &[crate::background::Queued]) {
    if lost.is_empty() {
        return;
    }
    let lost: Vec<String> = lost.iter().map(|q| q.text.clone()).collect();
    let lost = lost.as_slice();
    let file = note_file(id, &undelivered_text(lost));
    crate::log::warn(
        "agents",
        &format!(
            "{} went with {} nobody was told of: {}",
            label(id),
            count_of(lost),
            file.map_or("not written".into(), |p| p.display().to_string())
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::delivery::{deliver_held, is_held};
    use crate::agents::notifier::Settled;
    use crate::agents::owed::{hand_over, owe_from};
    use crate::agents::persist::{chats_now, conversation_of, Opening};
    use crate::agents::read::transcripts;
    use crate::agents::registry::{reg, register};
    use crate::agents::spawn::chat_starting;
    use crate::agents::teardown::forget;
    use crate::agents::tests::{back, first_turn_cut, lost_helper, notice_for, take_up};
    use crate::agents::wait::wait;
    use crate::provider::Provider;
    use crate::throttle::now_ms;

    #[test]
    fn what_never_reached_a_helper_goes_only_to_its_sender_and_a_cancel_brings_it_back() {
        register("qs-child", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("qs-parent"), false, serde_json::json!({}));
        crate::background::take_up_for_test("qs-child", "C:\\p", 3, &[], None);
        crate::background::lose_for_test("qs-child", "qs-a", "a's private words");
        crate::background::lose_for_test("qs-child", "qs-b", "b's words");
        owe_from("qs-b", "qs-child", 3, 0);
        let line = notice_for("qs-b", "qs-child", 4).0.unwrap();
        assert!(line.contains("Not delivered: 1 message"), "{line}");
        let file = line.split(", in ").nth(1).and_then(|l| l.split(';').next()).unwrap();
        let held = std::fs::read_to_string(file).unwrap();
        let _ = std::fs::remove_file(file);
        assert!(held.contains("b's words") && !held.contains("a's private"), "{held}");
        assert!(undelivered_note("qs-nobody", "qs-child").is_none(), "another reader hears nothing of it");
        assert!(undelivered_note("qs-parent", "qs-child").is_none(), "nor does its parent, of what others sent");

        // Handed over, it is gone; a cancel crossing the response brings it back.
        let of_a = || crate::background::undelivered_for("qs-child", "qs-a", false);
        assert_eq!(of_a(), ["a's private words"]);
        let undo = hand_over("qs-a", "qs-child", 5, None);
        let later = hand_over("qs-a", "qs-child", 5, None);
        drop(later);
        assert!(of_a().is_empty());
        assert_eq!(crate::background::undelivered_for("qs-child", "qs-b", false), ["b's words"], "b's stays");
        undo();
        assert_eq!(of_a(), ["a's private words"]);

        crate::background::remove("qs-child");
        forget("qs-child");
        reg().lock().unwrap().owed.retain(|(_, s), _| s != "qs-child");
    }

    /// What the file a notice names holds, the notice's words before the name given.
    fn file_after(line: &str, before: &str) -> String {
        let at = line.find(before).unwrap_or_else(|| panic!("{before:?} not in {line}")) + before.len();
        let path = line[at..].split(".md").next().unwrap();
        let path = format!("{path}.md");
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        text
    }

    #[test]
    fn a_visible_helper_back_with_nothing_never_got_started_and_gives_back_what_it_was_asked() {
        // Both CLIs alike. One from a file an older core wrote has the note on it.
        for (id, provider, kept) in [
            ("fs-claude", Provider::Claude, "the whole job\nword for word".to_string()),
            ("fs-codex", Provider::Codex, crate::pty::codex_helper_prompt("fs-parent", "the whole job\nword for word")),
        ] {
            first_turn_cut(id, "fs-parent", provider, &kept, &["and a follow-up"]);
            back(id, "fs-parent", provider);
            chat_starting(id, false);
            crate::activity::between_turns_for_test(id);
            let (line, how) = notice_for("fs-parent", id, 1);
            let line = line.unwrap();
            assert!(matches!(how, Settled::Answered));
            assert!(
                line.contains("never got started") && line.contains("luna_send starts it afresh") && !line.contains("did NOT finish"),
                "{line}"
            );
            let held = file_after(&line, "its prompt and 1 queued message are in ");
            assert!(held.find("the whole job\nword for word").unwrap() < held.find("and a follow-up").unwrap(), "{held}");
            assert!(!held.contains("A note from Luna"), "what it asked, not what Luna added: {held}");

            // Only its own sender hears of it, and read or waited on, it goes with
            // that response; a cancel crossing it brings it back.
            assert!(undelivered_note("fs-nobody", id).is_none());
            let note = undelivered_note("fs-parent", id).unwrap();
            assert!(note.contains("never got started") && note.contains("its prompt and 1 queued message are in "), "{note}");
            let undo = hand_over("fs-parent", id, 1, None);
            assert!(undelivered_note("fs-parent", id).is_none() && !is_held("fs-parent", id));
            assert!(!first_turn_lost(crate::pty::manager(), id), "told: from here on it is as any other");
            undo();
            assert!(undelivered_note("fs-parent", id).is_some() && is_held("fs-parent", id));

            reg().lock().unwrap().held.retain(|h| h.id != id);
            forget(id);
        }
    }

    #[test]
    fn what_a_visible_helper_lost_stays_lost_through_a_turn_it_was_not_asked_for_and_across_a_core() {
        first_turn_cut("fl-chat", "fl-parent", Provider::Claude, "the job", &[]);
        back("fl-chat", "fl-parent", Provider::Claude);
        chat_starting("fl-chat", false);
        // A person typed into it, and a turn on that ended: the job is no nearer.
        crate::activity::keep_count("fl-chat", 5);
        assert!(first_turn_lost(crate::pty::manager(), "fl-chat"));
        assert_eq!(
            chats_now().chats.into_iter().find(|c| c.id == "fl-chat").unwrap().ties.opening.as_deref(),
            Some("the job"),
            "a count does not settle this"
        );
        // A core going before its parent hears.
        let saved = chats_now().chats.into_iter().find(|c| c.id == "fl-chat").unwrap();
        assert!(saved.ties.opening_lost);
        forget("fl-chat");
        take_up(vec![saved], Vec::new(), Vec::new());
        assert!(reg().lock().unwrap().openings["fl-chat"].lost);
        forget("fl-chat");
    }

    #[test]
    fn a_visible_helper_with_a_conversation_gets_the_plain_notice() {
        let path = std::env::temp_dir().join(format!("luna-fc-{}.jsonl", std::process::id()));
        std::fs::write(&path, "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"the job\"}}\n").unwrap();
        first_turn_cut("fc-chat", "fc-parent", Provider::Claude, "the job", &[]);
        transcripts().lock().unwrap().insert("fc-chat".into(), path.clone());
        back("fc-chat", "fc-parent", Provider::Claude);
        chat_starting("fc-chat", true);
        crate::activity::between_turns_for_test("fc-chat");
        assert!(!first_turn_lost(crate::pty::manager(), "fc-chat"));
        let line = notice_for("fc-parent", "fc-chat", 1).0.unwrap();
        assert!(
            line.contains("did NOT finish") && line.contains("luna_send has it carry on") && !line.contains("never got started"),
            "{line}"
        );
        assert!(undelivered_note("fc-parent", "fc-chat").is_none(), "its conversation holds it");
        forget("fc-chat");
        let _ = std::fs::remove_file(&path);

        // A first turn no core has been near: a CLI started afresh on nothing is
        // no loss of anything.
        register("fc-first", Provider::Claude, "C:\\p", "C:\\acc\\anthropic\\one", Some("fc-parent"), false, serde_json::json!({}));
        reg().lock().unwrap().openings.insert("fc-first".into(), Opening { text: "go".into(), floor: 0, lost: false, at_ms: 0 });
        chat_starting("fc-first", false);
        assert!(!first_turn_lost(crate::pty::manager(), "fc-first"));
        forget("fc-first");
    }

    #[test]
    fn a_visible_helper_not_back_in_time_is_told_of_by_what_is_on_disk() {
        // No conversation on disk: its text stays for this notice, not dropped as
        // lost to a session that went (`deliver_held`).
        first_turn_cut("fb-none", "fb-parent", Provider::Codex, "the whole job", &["and a follow-up"]);
        back("fb-none", "fb-parent", Provider::Codex);
        deliver_held(crate::pty::manager());
        assert!(is_held("fb-parent", "fb-none"));
        reg().lock().unwrap().worktrees_before.insert("fb-none".into(), Vec::new());
        let (line, how) = notice_for("fb-parent", "fb-none", 1);
        let line = line.unwrap();
        assert!(matches!(how, Settled::Exited));
        assert!(line.contains("never got started") && line.contains("has not started again") && line.contains("luna_delete"), "{line}");
        assert!(!line.contains("luna_send"), "no sending to one not running: {line}");
        let held = file_after(&line, "its prompt and 1 queued message are in ");
        assert!(held.find("the whole job").unwrap() < held.find("and a follow-up").unwrap(), "{held}");
        assert!(!reg().lock().unwrap().worktrees_before.contains_key("fb-none"), "told of what it began, it makes its own if opened");
        reg().lock().unwrap().held.retain(|h| h.id != "fb-none");
        forget("fb-none");

        // One on disk: it comes back to that when opened.
        let path = std::env::temp_dir().join(format!("luna-fb-{}.jsonl", std::process::id()));
        std::fs::write(&path, "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"the job\"}}\n").unwrap();
        first_turn_cut("fb-said", "fb-parent", Provider::Claude, "the job", &[]);
        transcripts().lock().unwrap().insert("fb-said".into(), path.clone());
        back("fb-said", "fb-parent", Provider::Claude);
        let line = notice_for("fb-parent", "fb-said", 1).0.unwrap();
        assert!(line.contains("did NOT finish") && line.contains("runs again once the user opens it"), "{line}");
        forget("fb-said");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_conversation_on_disk_is_gone_back_to_and_never_told_of_as_never_started() {
        let root = std::env::temp_dir().join(format!("luna-rc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let folder = root.join("proj").to_string_lossy().into_owned();
        std::fs::create_dir_all(&folder).unwrap();
        let account = root.join("anthropic").to_string_lossy().into_owned();
        // Started by no window yet, a look at it goes by where it was to work.
        let on_disk = |id: &str, provider: Provider, account: &str| {
            register(id, provider, &folder, account, Some("rc-parent"), false, serde_json::json!({}));
        };
        let user_line = |text: &str| format!("{}\n", serde_json::json!({ "type": "user", "message": { "role": "user", "content": text } }));

        // Claude Code: the window has not heard, and what it was asked is in one begun since.
        first_turn_cut("rc-claude", "rc-parent", Provider::Claude, "the whole job", &[]);
        on_disk("rc-claude", Provider::Claude, &account);
        assert_eq!(conversation_of("rc-claude", Provider::Claude, &folder, &account), None, "nothing there yet");
        assert!(first_turn_lost(crate::pty::manager(), "rc-claude"));
        let path = crate::claude::session::transcript_file(&account, &folder, "11111111-aaaa");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, user_line("some other job")).unwrap();
        assert_eq!(conversation_of("rc-claude", Provider::Claude, &folder, &account), None, "not what it was asked: another session's");
        std::fs::write(&path, user_line("the  whole\njob")).unwrap();
        assert!(!first_turn_lost(crate::pty::manager(), "rc-claude"), "not back, with a conversation to come back to");
        let line = notice_for("rc-parent", "rc-claude", 1).0.unwrap();
        assert!(line.contains("did NOT finish") && !line.contains("never got started"), "{line}");
        reg().lock().unwrap().openings.get_mut("rc-claude").unwrap().at_ms = now_ms() + 3_600_000;
        assert_eq!(conversation_of("rc-claude", Provider::Claude, &folder, &account), None, "begun before it was asked: an older one's");
        reg().lock().unwrap().openings.get_mut("rc-claude").unwrap().at_ms = 0;
        // Another session's, known to be, is not to be taken for it.
        transcripts().lock().unwrap().insert("rc-other".into(), path.clone());
        assert_eq!(conversation_of("rc-claude", Provider::Claude, &folder, &account), None);
        transcripts().lock().unwrap().remove("rc-other");
        assert_eq!(conversation_of("rc-claude", Provider::Claude, &folder, &account).as_deref(), Some("11111111-aaaa"));
        // It goes back: no prompt is lost, and the caller hears it did not finish.
        chat_starting("rc-claude", true);
        assert!(!reg().lock().unwrap().openings["rc-claude"].lost);
        assert!(undelivered_note("rc-parent", "rc-claude").is_none());
        forget("rc-claude");

        // Whatever the last core knew of: that one, whatever it holds of the prompt.
        first_turn_cut("rc-kept", "rc-parent", Provider::Claude, "a prompt not in it", &[]);
        on_disk("rc-kept", Provider::Claude, &account);
        transcripts().lock().unwrap().insert("rc-kept".into(), path.clone());
        assert_eq!(conversation_of("rc-kept", Provider::Claude, &folder, &account).as_deref(), Some("11111111-aaaa"));
        forget("rc-kept");

        // Codex: by its rollout, in the folder or a worktree of it.
        let account = root.join("openai").to_string_lossy().into_owned();
        first_turn_cut("rc-codex", "rc-parent", Provider::Codex, "the whole job", &[]);
        on_disk("rc-codex", Provider::Codex, &account);
        assert_eq!(conversation_of("rc-codex", Provider::Codex, &folder, &account), None);
        let now = chrono::Local::now();
        let dir = std::path::Path::new(&account)
            .join("sessions")
            .join(now.format("%Y").to_string())
            .join(now.format("%m").to_string())
            .join(now.format("%d").to_string());
        std::fs::create_dir_all(&dir).unwrap();
        let rollout = |source: &str, cwd: &str, said: &str, thread: &str| {
            let file = dir.join(format!("rollout-{}-{thread}.jsonl", now.format("%Y-%m-%dT%H-%M-%S")));
            let meta = serde_json::json!({ "type": "session_meta", "payload": { "id": thread, "cwd": cwd, "source": source } });
            let user = serde_json::json!({ "type": "response_item", "payload": { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": said }] } });
            std::fs::write(file, format!("{meta}\n{user}\n")).unwrap();
        };
        rollout("exec", &folder, "the whole job", "aaaa0000-exec");
        rollout("cli", &root.join("elsewhere").to_string_lossy(), "the whole job", "aaaa0000-away");
        rollout("cli", &folder, "another job", "aaaa0000-else");
        assert_eq!(conversation_of("rc-codex", Provider::Codex, &folder, &account), None, "a helper's, another folder's, another's job");
        rollout("cli", &folder, &crate::pty::codex_helper_prompt("rc-parent", "the whole job"), "aaaa0000-mine");
        assert_eq!(
            conversation_of("rc-codex", Provider::Codex, &folder, &account).as_deref(),
            Some("aaaa0000-mine"),
            "with its note on it"
        );
        forget("rc-codex");
        transcripts().lock().unwrap().retain(|k, _| !k.starts_with("rc-"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_wait_on_a_lost_first_turn_answers_at_once_though_text_is_held_for_it() {
        let dir = std::env::temp_dir().to_string_lossy().into_owned();
        lost_helper("lw-parent", "lw-chat", &dir, &["and a follow-up"]);
        let key = ("lw-parent".to_string(), "lw-chat".to_string());
        // Held text clears only by going in, and this one is kept back: the wait used
        // to sit its whole time out on it, with nothing to show.
        let call = crate::mcp::Call::new();
        let t0 = std::time::Instant::now();
        let w = wait("lw-parent", "lw-chat", "turn_done", 30, None, &call).unwrap();
        assert!(t0.elapsed() < std::time::Duration::from_secs(5), "{:?}", t0.elapsed());
        assert!(w.failed.as_deref().is_some_and(|f| f.contains("never got started")), "{:?}", w.failed);
        let note = w.undelivered.clone().unwrap();
        assert!(note.contains("its prompt and 1 queued message are in "), "{note}");
        let held = file_after(&note, "its prompt and 1 queued message are in ");
        assert!(held.find("the whole job").unwrap() < held.find("and a follow-up").unwrap(), "{held}");
        // Handed over with the debt, so no notice says it again.
        assert!(!reg().lock().unwrap().owed.contains_key(&key));
        assert!(!is_held("lw-parent", "lw-chat") && !first_turn_lost(crate::pty::manager(), "lw-chat"));
        // A cancel crossing the response brings back both, together.
        call.cancel();
        assert!(reg().lock().unwrap().owed.contains_key(&key) && is_held("lw-parent", "lw-chat"));
        assert!(first_turn_lost(crate::pty::manager(), "lw-chat"));
        // A cancel first hands over nothing.
        let call = crate::mcp::Call::new();
        call.cancel();
        assert!(wait("lw-parent", "lw-chat", "turn_done", 30, None, &call).is_err());
        assert!(reg().lock().unwrap().owed.contains_key(&key) && is_held("lw-parent", "lw-chat"));
        // Not running, it is all there is to wait for, whatever the wait was for.
        let w = wait("lw-parent", "lw-chat", "waiting", 1, None, &crate::mcp::Call::new()).unwrap();
        assert!(w.undelivered.is_some());
        assert!(!still_all_lost(crate::pty::manager(), "lw-nobody", "lw-chat"), "only its parent's to have");

        // Asked something since that went in, it has started on that: the wait is for
        // its answer, and text held since goes in as any does.
        first_turn_cut("lw-sent", "lw-parent", Provider::Claude, "the whole job", &["a later one"]);
        back("lw-sent", "lw-parent", Provider::Claude);
        chat_starting("lw-sent", false);
        assert!(still_all_lost(crate::pty::manager(), "lw-parent", "lw-sent"));
        owe_from("lw-parent", "lw-sent", 1, 0);
        assert!(!still_all_lost(crate::pty::manager(), "lw-parent", "lw-sent"));
        assert!(
            first_turn_lost(crate::pty::manager(), "lw-sent"),
            "what it was first asked is lost all the same, and told of with that answer"
        );
        assert!(undelivered_for("lw-sent", "lw-parent", true).iter().any(|t| t == "the whole job"));
        reg().lock().unwrap().held.retain(|h| h.id != "lw-chat" && h.id != "lw-sent");
        forget("lw-sent");
        forget("lw-chat");
        crate::background::gone_for_test("lw-parent");
        forget("lw-parent");
    }
}
