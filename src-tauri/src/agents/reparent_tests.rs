use super::*;
use crate::agents::owed::owe_from;
use crate::agents::persist::{tie_back, ties_of, Ties};
use crate::agents::registry::{authorize, authorize_send, in_scope, ownership_steady, parent_of, register, set_name};
use crate::agents::teardown::{forget, news_for, Auto};
use crate::provider::Provider;

const ACCOUNT: &str = "C:/acc/anthropic/one";

/// A chat of the person's, with Luna's tools or without, in `mode`.
fn chat(id: &str, tools: bool, mode: &str) {
    register(id, Provider::Claude, "C:/p", ACCOUNT, None, tools, serde_json::json!({ "permissionMode": mode }));
}

/// A helper of `parent` running in the background, between turns, on `provider`.
fn helper(id: &str, parent: &str, provider: Provider) {
    let settings = match provider {
        Provider::Claude => serde_json::json!({ "background": true, "permissionMode": "acceptEdits" }),
        Provider::Codex => serde_json::json!({ "background": true, "sandbox": "workspace-write", "approval": "never" }),
    };
    let account = if provider == Provider::Codex { "C:/acc/openai/two" } else { ACCOUNT };
    register(id, provider, "C:/p", account, Some(parent), false, settings);
    crate::background::idle_for_test(id, provider, 2, false);
    crate::background::set_parent(id, Some(parent.to_string()));
}

fn owes(caller: &str, id: &str) -> bool {
    reg().lock().unwrap().owed.contains_key(&(caller.to_string(), id.to_string()))
}

fn clean(ids: &[&str]) {
    for id in ids {
        crate::background::gone_for_test(id);
        reg().lock().unwrap().news.retain(|(c, _)| c != id);
        forget(id);
    }
}

/// A helper moves with its line, on either CLI: only its parent changes, its old
/// chat is owed nothing more and hears once, its later writes are refused naming
/// the new chat while reads and waits with scope "all" go on, and what the move
/// left survives a core's restart through the helper's ties.
#[test]
fn a_helper_moves_with_its_line_and_its_ties_come_back() {
    for (provider, tag) in [(Provider::Claude, "cl"), (Provider::Codex, "cx")] {
        let [a, b, h, g] = ["a", "b", "h", "g"].map(|s| format!("mv-{tag}-{s}"));
        chat(&a, true, "bypassPermissions");
        chat(&b, true, "bypassPermissions");
        set_name(&b, "Release");
        helper(&h, &a, provider);
        helper(&g, &h, provider);
        set_name(&h, "the job");
        owe_from(&a, &h, 2, 0);
        owe_from(&a, &g, 2, 0);
        owe_from(&h, &g, 2, 0);

        let moved = move_helper(&h, &b).unwrap();
        assert_eq!((moved.from.as_deref(), moved.sessions.clone()), (Some(a.as_str()), vec![h.clone(), g.clone()]));
        assert!(!moved.owed, "idle with nothing queued or asked: nothing to owe");
        assert_eq!(parent_of(&h).as_deref(), Some(b.as_str()));
        assert_eq!(parent_of(&g).as_deref(), Some(h.as_str()), "its own helpers stay its");
        assert_eq!(crate::background::told_parent_for_test(&h).as_deref(), Some(b.as_str()), "its next turn's parent");
        assert!(!owes(&a, &h) && !owes(&a, &g), "the old chat is owed nothing more by its line");
        assert!(owes(&h, &g), "what is owed within the line stands");
        let told = news_for(&a);
        assert_eq!(told.len(), 1, "{told:?}");
        assert!(
            told[0].contains(&format!("moved your helper {h} \"the job\" with the helper under it to chat \"Release\" ({b})")),
            "{}",
            told[0]
        );

        // The old chat: refused writes and stop or delete, naming the new chat;
        // reads and waits with scope "all" open, without it refused the same way.
        for e in [authorize(&a, &h).err().unwrap(), authorize_send(&a, &h).err().unwrap()] {
            assert!(e.contains(&format!("the person moved {h} to chat \"Release\" ({b})")) && e.contains("no longer yours"), "{e}");
        }
        for verb in ["send to", "answer"] {
            let e = in_scope(&a, &g, Some("all"), verb).unwrap_err();
            assert!(e.contains(&format!("moved {h}, and {g} with it,")), "{e}");
        }
        for verb in ["read", "wait on"] {
            assert_eq!(in_scope(&a, &h, Some("all"), verb), Ok(()));
            assert!(in_scope(&a, &h, None, verb).unwrap_err().contains("the person moved"), "a wait of its own ends");
        }
        assert!(authorize_send(&b, &g).is_ok() && authorize(&b, &h).is_ok(), "the new chat's now");

        // The window's row.
        let o = ownership_of(&h);
        assert_eq!(o.top.as_deref(), Some(b.as_str()));
        assert_eq!(o.moved.as_ref().map(|m| (m.from.clone(), m.to.clone())), Some((Some(a.clone()), b.clone())));
        assert_eq!(ownership_of(&g).top.as_deref(), Some(b.as_str()), "and its helpers' line");

        // Its next turn is told once.
        let note = move_note(&h).expect("not told yet");
        assert!(note.contains(&format!("to chat \"Release\" ({b})")) && note.contains(&format!("from chat {a}")), "{note}");

        // Across a core: its ties bring the move back, still to be told.
        let ties: Ties = serde_json::from_value(serde_json::to_value(ties_of(&h).unwrap()).unwrap()).unwrap();
        assert_eq!(ties.parent.as_deref(), Some(b.as_str()));
        {
            let mut r = reg().lock().unwrap();
            r.known.remove(&h);
            r.moved.remove(&h);
        }
        tie_back(&h, provider, "C:/p", ACCOUNT, ties);
        assert_eq!(parent_of(&h).as_deref(), Some(b.as_str()));
        assert!(authorize_send(&a, &h).err().unwrap().contains("the person moved"), "refused as before");
        assert!(move_note(&h).is_some());
        move_told(&h);
        assert_eq!(move_note(&h), None, "a turn had it");
        clean(&[&g, &h, &a, &b]);
    }
}

/// Every refusal leaves everything as it was: nothing moved, nobody told.
#[test]
fn a_move_is_refused_whole_naming_why() {
    let [a, b, plain, low, h, g, lead] = ["a", "b", "plain", "low", "h", "g", "lead"].map(|s| format!("mr-{s}"));
    chat(&a, true, "bypassPermissions");
    chat(&b, true, "bypassPermissions");
    chat(&plain, false, "bypassPermissions");
    chat(&low, true, "plan");
    helper(&h, &a, Provider::Claude);
    helper(&g, &h, Provider::Codex);
    register(&lead, Provider::Claude, "C:/p", ACCOUNT, Some(&b), true, serde_json::json!({ "background": true, "permissionMode": "auto" }));
    let refused = |id: &str, to: &str, why: &str| {
        let e = move_helper(id, to).unwrap_err();
        assert!(e.contains(why), "{id} -> {to}: {e}");
        assert_eq!(parent_of(&h).as_deref(), Some(a.as_str()), "nothing moved");
        assert!(news_for(&a).is_empty() && !reg().lock().unwrap().moved.contains_key(&h));
    };
    refused(&h, &plain, "runs without Luna's tools");
    refused(&h, &g, "a helper does not move into its own line");
    refused(&h, &lead, "is a lead or a helper");
    refused(&h, &low, "above chat");
    refused(&h, &a, "already");
    refused(&h, "mr-nobody", "not running under Luna");
    refused(&g, &b, &format!("Move {h} and {g} goes with it"));
    refused(&a, &b, "a chat of the person's, not a helper");
    crate::background::promoting_for_test(&h, true);
    refused(&h, &b, "held back (the person is opening it as a chat of their own)");
    crate::background::promoting_for_test(&h, false);
    // The places it would take: chat b has the lead and six more helpers, 7 of
    // 8 places, and h brings itself and g.
    let kids: Vec<String> = (0..6).map(|i| format!("mr-kid-{i}")).collect();
    for k in &kids {
        register(k, Provider::Claude, "C:/p", ACCOUNT, Some(&b), false, serde_json::json!({ "background": true }));
    }
    refused(&h, &b, "has 7 of its 8 places in use");
    // One of them answered and waits out autoDelete: no place, and room for both.
    reg().lock().unwrap().auto.insert(kids[0].clone(), Auto { released: true, ..Auto::default() });

    // The window's menu says the same, chat by chat.
    let menu = move_targets(&h);
    assert_eq!(menu.blocked, None);
    let why = |to: &str| menu.targets.iter().find(|t| t.id == to).map(|t| (t.ok, t.why.clone().unwrap_or_default()));
    assert_eq!(why(&b), Some((true, String::new())));
    assert!(why(&plain).unwrap().1.contains("without Luna's tools"));
    assert!(why(&low).unwrap().1.contains("above chat"));
    assert!(why(&a).unwrap().1.contains("already"));
    assert_eq!(why(&lead), None, "a lead is no chat to list");
    assert!(move_targets(&g).blocked.unwrap().contains("goes with it"));
    crate::background::promoting_for_test(&h, true);
    assert!(move_targets(&h).blocked.unwrap().contains("held back"));
    crate::background::promoting_for_test(&h, false);

    assert!(move_helper(&h, &b).is_ok(), "with a place freed");
    reg().lock().unwrap().auto.remove(&kids[0]);
    let mut all: Vec<&str> = kids.iter().map(String::as_str).collect();
    all.extend([g.as_str(), h.as_str(), lead.as_str(), a.as_str(), b.as_str(), plain.as_str(), low.as_str()]);
    clean(&all);
}

/// A send from the old chat that passed its check before the move has its debt
/// in before the move goes on, which closes it; one after the move is refused
/// naming the new chat. Either way the old chat is owed nothing, and the new
/// one is owed what the helper is at work on. Both CLIs alike.
#[test]
fn a_move_racing_a_send_leaves_the_old_chat_owed_nothing() {
    for (provider, tag) in [(Provider::Claude, "cl"), (Provider::Codex, "cx")] {
        let [a, b, h] = ["a", "b", "h"].map(|s| format!("rs-{tag}-{s}"));
        chat(&a, true, "bypassPermissions");
        chat(&b, true, "bypassPermissions");
        helper(&h, &a, provider);
        crate::background::in_turn_for_test(&h);
        // A send under way: checked, its text not in yet. The move waits for it.
        let held = ownership_steady();
        let mover = {
            let (h, b) = (h.clone(), b.clone());
            std::thread::spawn(move || move_helper(&h, &b))
        };
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert_eq!(parent_of(&h).as_deref(), Some(a.as_str()), "the move waits for the send");
        let placed = crate::background::send(&h, "one more thing", Some(&a)).unwrap();
        assert_eq!(placed, crate::background::Placed::Queued);
        owe_from(&a, &h, 2, 0);
        drop(held);
        let moved = mover.join().unwrap().unwrap();
        assert!(moved.owed, "mid-turn: the new chat is owed its answer");
        assert!(!owes(&a, &h), "the send's debt went with the move");
        assert!(owes(&b, &h));
        let told = news_for(&a);
        assert!(told[0].contains("The message you had queued for it still goes in"), "{}", told[0]);
        assert_eq!(crate::background::queued_for_test(&h), ["one more thing"], "plain text queued goes in");

        // After the move, the old chat's send is refused naming the new chat.
        let e = crate::agents::send(&a, &h, "and this", false, None, true).err().unwrap();
        assert!(e.contains("the person moved") && e.contains(&b), "{e}");
        assert!(!owes(&a, &h) && crate::background::queued_for_test(&h).len() == 1);
        clean(&[&h, &a, &b]);
    }
}

/// An answer the old chat gives as the move comes: whichever goes first, the
/// move waits for the answer's debt and closes it, or the answer is refused.
#[test]
fn a_move_racing_an_answer_leaves_the_old_chat_owed_nothing() {
    let [a, b, h] = ["a", "b", "h"].map(|s| format!("ra-{s}"));
    chat(&a, true, "bypassPermissions");
    chat(&b, true, "bypassPermissions");
    helper(&h, &a, Provider::Claude);
    crate::background::in_turn_for_test(&h);
    crate::background::hold_question_for_test(&h);
    let answering = {
        let (a, h) = (a.clone(), h.clone());
        std::thread::spawn(move || crate::agents::answer(&a, &h, None, &["Blue".to_string()], None, None, false))
    };
    // The answer is in its settle, holding whose the helper is, when the move comes.
    std::thread::sleep(std::time::Duration::from_millis(500));
    let moved = move_helper(&h, &b).unwrap();
    assert!(answering.join().unwrap().is_ok(), "it went in before the move");
    assert!(moved.owed && owes(&b, &h) && !owes(&a, &h), "its debt closed by the move, the new chat's open");
    // Asked again after the move: refused, the new chat named.
    crate::background::hold_question_for_test(&h);
    let e = crate::agents::answer(&a, &h, None, &["Red".to_string()], None, None, false).err().unwrap();
    assert!(e.contains("the person moved") && e.contains(&b), "{e}");
    clean(&[&h, &a, &b]);
}

/// The new chat is owed an answer the helper has to give: a turn running, text
/// queued, a question held; not one that is idle with nothing coming.
#[test]
fn the_new_chat_is_owed_what_the_helper_is_at_work_on() {
    let [a, b] = ["a", "b"].map(|s| format!("wo-{s}"));
    chat(&a, true, "bypassPermissions");
    chat(&b, true, "bypassPermissions");
    for (case, at_work) in [("idle", false), ("turn", true), ("queue", true), ("ask", true)] {
        let h = format!("wo-{case}");
        helper(&h, &a, Provider::Codex);
        match case {
            "turn" => crate::background::in_turn_for_test(&h),
            "queue" => {
                crate::background::promoting_for_test(&h, true);
                crate::background::send(&h, "queued", Some(&a)).unwrap();
                crate::background::promoting_for_test(&h, false);
            }
            "ask" => crate::background::hold_for_test(&h),
            _ => {}
        }
        owe_from(&a, &h, 2, 0);
        let moved = move_helper(&h, &b).unwrap();
        assert_eq!((moved.owed, owes(&b, &h)), (at_work, at_work), "{case}");
        if at_work {
            assert_eq!(reg().lock().unwrap().owed[&(b.clone(), h.clone())].baseline, 2, "at the count it stands at");
        }
        assert!(!owes(&a, &h), "{case}");
        clean(&[&h]);
    }
    clean(&[&a, &b]);
}

/// An `autoDelete` its chat's going switched on is switched off by a move to a
/// chat, which is there to delete it; one its spawner asked for stays. A helper
/// with no chat above it moves the same way, and nobody is told.
#[test]
fn a_move_switches_off_only_the_auto_delete_its_chats_going_set() {
    let [gone, b, orphan, asked] = ["gone", "b", "orphan", "asked"].map(|s| format!("ad-{s}"));
    chat(&gone, true, "bypassPermissions");
    chat(&b, true, "bypassPermissions");
    helper(&orphan, &gone, Provider::Claude);
    helper(&asked, &b, Provider::Claude);
    reg().lock().unwrap().auto.insert(asked.clone(), Auto::default());
    forget(&gone);
    assert!(reg().lock().unwrap().known[&orphan].orphan);
    assert!(ties_of(&orphan).unwrap().auto_delete_passed_up, "kept apart across a core");
    let e = authorize_send(&b, &orphan).err().unwrap();
    assert!(e.contains("has no chat above it"), "{e}");

    let moved = move_helper(&orphan, &b).unwrap();
    assert!(moved.auto_delete_off && moved.from.is_none());
    {
        let r = reg().lock().unwrap();
        assert!(!r.auto.contains_key(&orphan) && !r.known[&orphan].orphan);
    }
    assert!(authorize_send(&b, &orphan).is_ok());
    assert!(move_note(&orphan).unwrap().contains("from no chat"));

    // Asked for at its spawn: moved on, it keeps it.
    chat(&gone, true, "bypassPermissions");
    let moved = move_helper(&asked, &gone).unwrap();
    assert!(!moved.auto_delete_off && reg().lock().unwrap().auto.contains_key(&asked));
    reg().lock().unwrap().auto.remove(&asked);
    clean(&[&orphan, &asked, &gone, &b]);
}

/// A person's chat refused a write into a helper not under it is offered as the
/// chat to move it to; a read refused, or a helper refused, offers nothing. A
/// move clears what it was offered, and so does the helper's going.
#[test]
fn a_refused_write_from_a_chat_suggests_moving_the_helper_there() {
    let [a, c, d, h, sib] = ["a", "c", "d", "h", "sib"].map(|s| format!("sg-{s}"));
    chat(&a, true, "bypassPermissions");
    chat(&c, true, "bypassPermissions");
    chat(&d, true, "bypassPermissions");
    helper(&h, &a, Provider::Claude);
    helper(&sib, &a, Provider::Claude);
    let asked = || ownership_of(&h).suggested.into_iter().map(|s| s.chat).collect::<Vec<_>>();
    assert!(in_scope(&c, &h, Some("all"), "send to").is_err());
    assert!(in_scope(&d, &h, None, "read").is_err(), "a read refused offers nothing");
    assert!(authorize_send(&sib, &h).is_err(), "nor a helper refused");
    assert_eq!(asked(), std::slice::from_ref(&c));
    assert!(authorize_send(&d, &h).is_err());
    assert_eq!(asked(), [c.clone(), d.clone()]);
    // A wait in flight from a chat not above it shows as watching.
    reg().lock().unwrap().waits.insert((d.clone(), h.clone()), 1);
    reg().lock().unwrap().waits.insert((a.clone(), h.clone()), 1);
    assert_eq!(ownership_of(&h).watched_by, std::slice::from_ref(&d), "its own chat is not watching, it manages");
    reg().lock().unwrap().waits.retain(|(_, s), _| *s != h);

    move_helper(&h, &c).unwrap();
    assert!(asked().is_empty(), "moved: nothing is offered any more");
    assert!(in_scope(&d, &h, Some("all"), "answer").is_err());
    assert_eq!(asked(), std::slice::from_ref(&d));
    forget(&d);
    assert!(asked().is_empty(), "a chat gone is offered no more");
    clean(&[&h, &sib, &a, &c]);
}
