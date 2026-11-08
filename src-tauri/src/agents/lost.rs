//! What never reached a helper: text queued for it that it went without, or a
//! first turn a core's going cut off before it got anywhere.

use crate::agents::read::MAX_TEXT;
use crate::agents::registry::{label, parent_of, reg};
use crate::agents::said::note_file;

/// Whether what `caller` is to hear of `id` is all there will be: it was asked, a core
/// went before it got started, and nothing has that but Luna (`background::never_started`).
/// No turn is coming to answer it.
pub(super) fn lost_whole(caller: &str, id: &str) -> bool {
    parent_of(id).as_deref() == Some(caller) && crate::background::never_started(id)
}

/// The count that hands over a debt on a first turn lost: past a turn that went with a
/// core. A question asked since is no such debt, and still waits for its turn.
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

/// What never reached `id` that `caller` is to hear of: what the helper keeps for
/// it (`background::undelivered_for`).
pub(super) fn undelivered_for(id: &str, caller: &str, parent: bool) -> Vec<String> {
    crate::background::undelivered_for(id, caller, parent)
}

/// What `take_undelivered` took off, for a cancel to put back.
pub(super) struct Undelivered {
    pub(super) queued: Vec<crate::background::Queued>,
    worktrees: Option<Vec<String>>,
}

/// Takes off what `undelivered_for` gives, and the worktrees the parent hears of
/// with it, `caller` having been told.
pub(super) fn take_undelivered(id: &str, caller: &str, parent: bool) -> Undelivered {
    let queued = crate::background::take_undelivered_for(id, caller, parent);
    let worktrees = if parent { reg().lock().unwrap_or_else(|e| e.into_inner()).worktrees_left.remove(id) } else { None };
    Undelivered { queued, worktrees }
}

pub(super) fn put_undelivered_back(id: &str, taken: Undelivered) {
    crate::background::put_back(id, taken.queued);
    if let Some(worktrees) = taken.worktrees {
        reg().lock().unwrap_or_else(|e| e.into_inner()).worktrees_left.insert(id.to_string(), worktrees);
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
        None => format!(" Not delivered: {what} queued for it; luna_message_list gives them, send again what still matters."),
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
    let whole = lost_whole(caller, id);
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
    use crate::agents::owed::{hand_over, owe_from};
    use crate::agents::registry::{reg, register};
    use crate::agents::teardown::forget;
    use crate::agents::test_support::notice_for;
    use crate::provider::Provider;

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
}
