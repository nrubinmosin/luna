use super::*;
use crate::agents::owed::owe_from;
use crate::agents::persist::{tie_back, ties_of, Ties};
use crate::agents::registry::{authorize, authorize_send, in_scope, parent_of, register, set_name};
use crate::agents::spawn::background::register_spawned;
use crate::agents::teardown::{forget, news_for, Auto};
use crate::agents::test_support::stop_at;
use crate::provider::Provider;

const ACCOUNT: &str = "C:/acc/anthropic/one";

/// A chat of the person's, with Luna's tools or without, in `mode`.
fn chat(id: &str, tools: bool, mode: &str) {
    register(id, Provider::Claude, "C:/p", ACCOUNT, None, tools, serde_json::json!({ "permissionMode": mode }));
}

/// A helper of `parent` running in the background, between turns, on `provider`.
fn helper(id: &str, parent: &str, provider: Provider) {
    registered(id, parent, provider);
    crate::background::idle_for_test(id, provider, 2, false);
    crate::background::set_parent_for_test(id, Some(parent.to_string()));
}

/// A helper of `parent` as its spawn registers it, before its background
/// record is made (spawn/background.rs `spawn_background`).
fn registered(id: &str, parent: &str, provider: Provider) {
    let (settings, account) = as_spawned(provider);
    register(id, provider, "C:/p", account, Some(parent), false, settings);
}

/// What a helper on `provider` is registered with, and its account.
fn as_spawned(provider: Provider) -> (serde_json::Value, &'static str) {
    match provider {
        Provider::Claude => (serde_json::json!({ "background": true, "permissionMode": "acceptEdits" }), ACCOUNT),
        Provider::Codex => {
            (serde_json::json!({ "background": true, "sandbox": "workspace-write", "approval": "never" }), "C:/acc/openai/two")
        }
    }
}

/// A helper of `parent` registered as its spawn registers it, through the
/// spawn's own hold and depth check (spawn/background.rs `register_spawned`),
/// with no CLI started.
fn spawned(id: &str, parent: &str, provider: Provider, tools: bool) -> Result<(), String> {
    let (settings, account) = as_spawned(provider);
    register_spawned(parent, id, provider, "C:/p", account, tools, settings).map(drop)
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
        let note = move_note(&h).expect("not told yet").text;
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
        let note = move_note(&h).expect("still to be told");
        move_told(&h, &note.of);
        assert!(move_note(&h).is_none(), "a turn had it");
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
    refused(&h, &a, "already");
    refused(&h, "mr-nobody", "not running under Luna");
    refused(&g, &b, &format!("Move {h} and {g} goes with it"));
    refused(&a, &b, "a chat of the person's, not a helper");
    crate::background::promoting_for_test(&h, true);
    refused(&h, &b, "held back (the person is opening it as a chat of their own)");
    crate::background::promoting_for_test(&h, false);
    // The places it would take: chat b has sixteen helpers at work, all 16 of
    // its places.
    let kids: Vec<String> = (0..16).map(|i| format!("mr-kid-{i}")).collect();
    for k in &kids {
        register(k, Provider::Claude, "C:/p", ACCOUNT, Some(&b), false, serde_json::json!({ "background": true }));
        crate::background::idle_for_test(k, Provider::Claude, 1, false);
        crate::background::set_parent_for_test(k, Some(b.clone()));
        crate::background::in_turn_for_test(k);
    }
    // h and g, between turns, take none of them.
    let to_b = || move_targets(&h).targets.into_iter().find(|t| t.id == b).map(|t| t.ok);
    assert_eq!(to_b(), Some(true), "a line between turns needs no place");
    // A spawn under way from h brings one.
    let spawn = u64::MAX - 17;
    crate::background::promised_for_test(spawn, Some(&h));
    refused(&h, &b, "has 16 of its 16 places in use");
    refused(&h, &b, "1 spawn(s) under way");
    // One of them ends its turn: room for it.
    crate::background::idle_for_test(&kids[0], Provider::Claude, 2, false);

    // The window's menu says the same, chat by chat.
    let menu = move_targets(&h);
    assert_eq!(menu.blocked, None);
    let why = |to: &str| menu.targets.iter().find(|t| t.id == to).map(|t| (t.ok, t.why.clone().unwrap_or_default()));
    assert_eq!(why(&b), Some((true, String::new())));
    assert!(why(&plain).unwrap().1.contains("without Luna's tools"));
    // A chat in a lower mode than the line's takes it all the same: modes are not compared.
    assert_eq!(why(&low), Some((true, String::new())));
    assert!(why(&a).unwrap().1.contains("already"));
    assert_eq!(why(&lead), None, "a lead is no chat to list");
    assert!(move_targets(&g).blocked.unwrap().contains("goes with it"));
    crate::background::promoting_for_test(&h, true);
    assert!(move_targets(&h).blocked.unwrap().contains("held back"));
    crate::background::promoting_for_test(&h, false);

    assert!(move_helper(&h, &b).is_ok(), "with a place freed");
    crate::background::promised_for_test(spawn, None);
    let mut all: Vec<&str> = kids.iter().map(String::as_str).collect();
    all.extend([g.as_str(), h.as_str(), lead.as_str(), a.as_str(), b.as_str(), plain.as_str(), low.as_str()]);
    clean(&all);
}

/// A send from the old chat that passed its check before the move has its debt
/// in before the move goes on, which closes it; one after the move is refused
/// naming the new chat. Either way the old chat is owed nothing, and the new
/// one is owed what the helper is at work on. Both CLIs alike. Through the
/// send itself, stopped between its check and its debt: without its hold on
/// whose the helper is, the move would run there and the debt land after it.
#[test]
fn a_move_racing_a_send_leaves_the_old_chat_owed_nothing() {
    for (provider, tag) in [(Provider::Claude, "cl"), (Provider::Codex, "cx")] {
        let [a, b, h] = ["a", "b", "h"].map(|s| format!("rs-{tag}-{s}"));
        chat(&a, true, "bypassPermissions");
        chat(&b, true, "bypassPermissions");
        helper(&h, &a, provider);
        crate::background::in_turn_for_test(&h);
        // A send under way: checked, its text not in yet.
        let stop = stop_at("send checked", &a);
        let sending = {
            let (a, h) = (a.clone(), h.clone());
            std::thread::spawn(move || crate::agents::send(&a, &h, "one more thing", false, None, true))
        };
        stop.wait();
        let mover = {
            let (h, b) = (h.clone(), b.clone());
            std::thread::spawn(move || move_helper(&h, &b))
        };
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(!mover.is_finished(), "the move waits for the send");
        assert_eq!(parent_of(&h).as_deref(), Some(a.as_str()));
        stop.go();
        assert!(sending.join().unwrap().unwrap().queued);
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
/// The answer is stopped past its check, so the move comes in its middle.
#[test]
fn a_move_racing_an_answer_leaves_the_old_chat_owed_nothing() {
    let [a, b, h] = ["a", "b", "h"].map(|s| format!("ra-{s}"));
    chat(&a, true, "bypassPermissions");
    chat(&b, true, "bypassPermissions");
    helper(&h, &a, Provider::Claude);
    crate::background::in_turn_for_test(&h);
    crate::background::hold_question_for_test(&h);
    let stop = stop_at("answer checked", &a);
    let answering = {
        let (a, h) = (a.clone(), h.clone());
        std::thread::spawn(move || crate::agents::answer(&a, &h, None, &["Blue".to_string()], None, None, false))
    };
    stop.wait();
    let mover = {
        let (h, b) = (h.clone(), b.clone());
        std::thread::spawn(move || move_helper(&h, &b))
    };
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(!mover.is_finished(), "the move waits for the answer");
    stop.go();
    assert!(answering.join().unwrap().is_ok(), "it went in before the move");
    let moved = mover.join().unwrap().unwrap();
    assert!(moved.owed && owes(&b, &h) && !owes(&a, &h), "its debt closed by the move, the new chat's open");
    // Asked again after the move: refused, the new chat named.
    crate::background::hold_question_for_test(&h);
    let e = crate::agents::answer(&a, &h, None, &["Red".to_string()], None, None, false).err().unwrap();
    assert!(e.contains("the person moved") && e.contains(&b), "{e}");
    clean(&[&h, &a, &b]);
}

/// A helper spawned under the line once the move has looked at it is held as
/// the rest are before anything moves, the move looking again; and one a
/// delete holds by then stops the move whole, as the line's own helpers would.
/// The spawn registers it as a spawn does (`register_spawned`), before the
/// move holds whose the helpers are. Once a move that goes through holds
/// that, a spawn's registration waits for it, and lands where it left the line.
#[test]
fn a_helper_spawned_under_the_line_mid_move_is_held_too_or_nothing_moves() {
    let [a, b, h, g, k] = ["a", "b", "h", "g", "k"].map(|s| format!("ml-{s}"));
    chat(&a, true, "bypassPermissions");
    chat(&b, true, "bypassPermissions");
    helper(&h, &a, Provider::Claude);
    let mid_move = |id: &str, then: &mut dyn FnMut()| {
        let looked = stop_at("move looked", &h);
        let held = stop_at("move held", &h);
        let mover = {
            let (h, b) = (h.clone(), b.clone());
            std::thread::spawn(move || move_helper(&h, &b))
        };
        looked.wait();
        assert_eq!(parent_of(&h).as_deref(), Some(a.as_str()));
        spawned(id, &h, Provider::Codex, false).unwrap();
        crate::background::idle_for_test(id, Provider::Codex, 2, false);
        crate::background::set_parent_for_test(id, Some(h.clone()));
        then();
        looked.go();
        held.wait();
        held.go();
        mover.join().unwrap()
    };
    let mut deleting = None;
    let moved = mid_move(&g, &mut || {}).map(|m| m.sessions);
    assert_eq!(moved, Ok(vec![h.clone(), g.clone()]), "held again, it with them");
    assert_eq!(parent_of(&g).as_deref(), Some(h.as_str()));
    move_helper(&h, &a).unwrap();
    // Taken for deletion between the move's look and its holds: not moved.
    let e = mid_move(&k, &mut || deleting = Some(crate::background::claim_delete(&k).unwrap())).unwrap_err();
    assert!(e.contains(&format!("{k} is held back (it is being deleted)")), "{e}");
    assert_eq!(parent_of(&h).as_deref(), Some(a.as_str()), "nothing moved");
    drop(deleting);
    // A move that holds whose the helpers are and goes through: a spawn under
    // its line waits for it, and its helper is not among those it moved.
    let late = "ml-late";
    let held = stop_at("move held", &h);
    let mover = {
        let (h, b) = (h.clone(), b.clone());
        std::thread::spawn(move || move_helper(&h, &b))
    };
    held.wait();
    let spawn = {
        let h = h.clone();
        std::thread::spawn(move || spawned(late, &h, Provider::Codex, false))
    };
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(!spawn.is_finished(), "a spawn's registration waits on the move");
    assert!(!crate::agents::registry::is_known(late), "nothing registered meanwhile");
    held.go();
    assert_eq!(mover.join().unwrap().map(|m| m.sessions), Ok(vec![h.clone(), g.clone(), k.clone()]));
    spawn.join().unwrap().unwrap();
    assert_eq!((parent_of(late).as_deref(), parent_of(&h).as_deref()), (Some(h.as_str()), Some(b.as_str())));
    clean(&[late, &k, &g, &h, &a, &b]);
}

/// A chat that goes as the move is through with the registry takes back what
/// it was to have: the helper is left with no chat in its next turn's spec as
/// in the registry, and owes the gone chat nothing.
#[test]
fn a_chat_gone_as_a_helper_moves_to_it_is_owed_nothing_and_named_nowhere() {
    let [a, b, h] = ["a", "b", "h"].map(|s| format!("mg-{s}"));
    chat(&a, true, "bypassPermissions");
    chat(&b, true, "bypassPermissions");
    helper(&h, &a, Provider::Codex);
    crate::background::in_turn_for_test(&h);
    let stop = stop_at("move changed", &h);
    let mover = {
        let (h, b) = (h.clone(), b.clone());
        std::thread::spawn(move || move_helper(&h, &b))
    };
    stop.wait();
    assert!(owes(&b, &h), "owed in the hold that moved it");
    forget(&b);
    stop.go();
    assert!(mover.join().unwrap().unwrap().owed);
    assert_eq!(parent_of(&h), None);
    assert_eq!(crate::background::told_parent_for_test(&h), None, "its next turn names no chat that is gone");
    let note = move_note(&h).expect("the move is still to be told").text;
    assert!(note.contains("which has gone since: no chat is above you now"), "{note}");
    assert!(!note.contains("you answer to that chat now"), "it names no gone chat as its own: {note}");
    assert!(!owes(&b, &h));
    clean(&[&h, &a]);
}

/// A chat's going that changed the registry before a move of its helper, and
/// writes the helper's next parent after it, leaves the move's: the record its
/// next turn starts from follows the registry whichever writes last.
#[test]
fn a_chats_going_written_after_a_move_of_its_helper_leaves_the_moves_parent() {
    let [a, b, h] = ["a", "b", "h"].map(|s| format!("rpf-{s}"));
    chat(&a, true, "bypassPermissions");
    chat(&b, true, "bypassPermissions");
    helper(&h, &a, Provider::Codex);
    let stop = stop_at("forgotten", &a);
    let forgetting = {
        let a = a.clone();
        std::thread::spawn(move || forget(&a))
    };
    stop.wait();
    assert_eq!(parent_of(&h), None, "the registry has let it go");
    move_helper(&h, &b).unwrap();
    assert_eq!(crate::background::told_parent_for_test(&h).as_deref(), Some(b.as_str()));
    stop.go();
    forgetting.join().unwrap();
    assert_eq!(parent_of(&h).as_deref(), Some(b.as_str()));
    assert_eq!(crate::background::told_parent_for_test(&h).as_deref(), Some(b.as_str()), "the late write of the chat's going lost");
    clean(&[&h, &b]);
}

/// A turn whose start was reserved before a move and whose command is made after
/// the move held the helper does not start: what it was to start on waits for
/// the turn after the move, under the new chat, with the move's note, and the
/// command text the old chat queued is judged then, against the new owner.
#[test]
fn a_turn_reserved_before_a_move_starts_after_it_on_what_the_new_chat_allows() {
    let [a, b, h] = ["a", "b", "h"].map(|s| format!("rpo-{s}"));
    chat(&a, true, "bypassPermissions");
    chat(&b, true, "bypassPermissions");
    helper(&h, &a, Provider::Claude);
    crate::background::in_turn_for_test(&h);
    for text in ["/compact", "plain"] {
        assert_eq!(crate::background::send(&h, text, Some(&a)).unwrap(), crate::background::Placed::Queued);
    }
    assert!(crate::agents::command_reach_now(&a, &h, "/compact").is_ok(), "its chat may send it a command");
    crate::background::reserved_for_test(&h);
    move_helper(&h, &b).unwrap();
    let told = news_for(&a);
    assert!(told[0].contains("The message you had queued for it still goes in"), "only the plain text is said to: {}", told[0]);
    assert!(!crate::background::launch_for_test(&h), "the reserved turn does not start");
    assert_eq!(crate::background::queued_for_test(&h), ["/compact", "plain"], "its batch waits, first in the queue");
    assert_eq!(crate::background::told_parent_for_test(&h).as_deref(), Some(b.as_str()));
    assert!(move_note(&h).unwrap().text.contains("you answer to that chat now"));
    assert_eq!(crate::background::settled_for_test(&h).as_deref(), Some("plain"), "the old chat's command is dropped");
    assert!(news_for(&a).iter().any(|l| l.contains("did not go in when its turn came")), "{:?}", news_for(&a));
    clean(&[&h, &a, &b]);
}

/// A turn whose command is made before a move holds the helper starts as made:
/// the move waits for its process, so the turn goes on as one begun before the
/// move, and nothing changes hands under its start.
#[test]
fn a_move_waits_for_a_turn_whose_command_is_made() {
    let [a, b, h] = ["a", "b", "h"].map(|s| format!("rpw-{s}"));
    chat(&a, true, "bypassPermissions");
    chat(&b, true, "bypassPermissions");
    helper(&h, &a, Provider::Codex);
    crate::background::in_turn_for_test(&h);
    assert_eq!(crate::background::send(&h, "plain", Some(&a)).unwrap(), crate::background::Placed::Queued);
    crate::background::reserved_for_test(&h);
    assert!(crate::background::launch_for_test(&h), "made before any move");
    let mover = {
        let (h, b) = (h.clone(), b.clone());
        std::thread::spawn(move || move_helper(&h, &b))
    };
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(!mover.is_finished(), "the move waits for the turn's process");
    assert_eq!(parent_of(&h).as_deref(), Some(a.as_str()), "and has changed nothing");
    crate::background::launch_failed_for_test(&h);
    mover.join().unwrap().unwrap();
    assert_eq!(parent_of(&h).as_deref(), Some(b.as_str()));
    clean(&[&h, &a, &b]);
}

/// A turn that ends while the move holds the helper starts nothing of what is
/// queued: that waits for the move to let go, so it starts under the new
/// parent, and queued command text is judged against the new owner.
#[test]
fn a_turn_ending_mid_move_starts_nothing_until_the_move_lets_go() {
    let [a, h] = ["a", "h"].map(|s| format!("te-{s}"));
    chat(&a, true, "bypassPermissions");
    helper(&h, &a, Provider::Claude);
    crate::background::in_turn_for_test(&h);
    assert_eq!(crate::background::send(&h, "queued", Some(&a)).unwrap(), crate::background::Placed::Queued);
    let claim = crate::background::claim_move(std::slice::from_ref(&h)).unwrap();
    crate::background::turn_ended_for_test(&h);
    assert_eq!(crate::background::queued_for_test(&h), ["queued"], "nothing started");
    assert_eq!(crate::background::work_and_count(&h), Some((true, 3)), "between turns, its queue still to come");
    // Gone before the claim lets go, whose release would start a real CLI.
    crate::background::gone_for_test(&h);
    drop(claim);
    clean(&[&h, &a]);
}

/// The note a move's next turn is given counts as told only once a turn's model
/// has answered a prompt that carried it, and only for the move it told of: a
/// move made since is told in its turn.
#[test]
fn a_move_is_told_once_a_turn_answered_its_note_and_only_that_move() {
    let [a, b, c, h] = ["a", "b", "c", "h"].map(|s| format!("mt-{s}"));
    for chat_id in [&a, &b, &c] {
        chat(chat_id, true, "bypassPermissions");
    }
    helper(&h, &a, Provider::Claude);
    move_helper(&h, &b).unwrap();
    let to_b = move_note(&h).unwrap();
    // The CLI went before its model did anything: told again next turn.
    crate::background::carried_move_for_test(&h, to_b.of.clone(), false);
    crate::background::turn_ended_for_test(&h);
    assert!(move_note(&h).is_some(), "not taken in");
    // Moved again while the turn that tells of the first runs.
    crate::background::carried_move_for_test(&h, to_b.of.clone(), true);
    move_helper(&h, &c).unwrap();
    crate::background::turn_ended_for_test(&h);
    let to_c = move_note(&h).expect("the second move is still to be told");
    assert!(to_c.text.contains(&format!("from chat {b} to chat {c}")), "{}", to_c.text);
    crate::background::carried_move_for_test(&h, to_c.of, true);
    crate::background::turn_ended_for_test(&h);
    assert!(move_note(&h).is_none(), "told");
    clean(&[&h, &a, &b, &c]);
}

/// A refused write into a helper's helper offers to move the helper it moves
/// with, the chat's own one, and the refusal says so: the helper's helper
/// alone could never move.
#[test]
fn a_refused_write_into_a_helpers_helper_suggests_moving_the_one_above() {
    let [a, c, h, g] = ["a", "c", "h", "g"].map(|s| format!("sr-{s}"));
    chat(&a, true, "bypassPermissions");
    chat(&c, true, "bypassPermissions");
    helper(&h, &a, Provider::Claude);
    helper(&g, &h, Provider::Codex);
    let e = authorize_send(&c, &g).err().unwrap();
    assert!(e.contains(&format!("move {h} here in Luna's window, and {g} goes with it")), "{e}");
    let asked = |id: &str| ownership_of(id).suggested.into_iter().map(|s| s.chat).collect::<Vec<_>>();
    assert_eq!(asked(&h), std::slice::from_ref(&c));
    assert!(asked(&g).is_empty(), "never offered where a move is refused");
    assert_eq!(move_helper(&h, &c).unwrap().sessions, [h.clone(), g.clone()]);
    clean(&[&g, &h, &a, &c]);
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
    assert!(move_note(&orphan).unwrap().text.contains("from no chat"));

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

/// A line left with no chat (its chat deleted, the helper above it left alone)
/// has no top: a helper is never named as the chat it hangs from.
#[test]
fn a_line_left_with_no_chat_hangs_from_no_chat() {
    let [a, h, g] = ["a", "h", "g"].map(|s| format!("nt-{s}"));
    chat(&a, true, "bypassPermissions");
    helper(&h, &a, Provider::Claude);
    helper(&g, &h, Provider::Claude);
    assert_eq!(ownership_of(&g).top.as_deref(), Some(a.as_str()));
    forget(&a);
    assert_eq!(ownership_of(&h).top, None, "parentless");
    assert_eq!(ownership_of(&g).top, None, "under a helper with no chat");
    reg().lock().unwrap().auto.remove(&h);
    clean(&[&g, &h]);
}

/// A chat's Delete dialog deletes only what still hangs from it: a helper moved
/// to another chat meanwhile (from another window) is refused and stays; one
/// its chat's going left with no chat, which may come first, still goes.
#[test]
fn a_chats_delete_spares_a_helper_moved_to_another_chat_meanwhile() {
    use crate::agents::teardown::delete_by_user;
    let [a, b, h, g, k] = ["a", "b", "h", "g", "k"].map(|s| format!("dl-{s}"));
    chat(&a, true, "bypassPermissions");
    chat(&b, true, "bypassPermissions");
    set_name(&b, "Release");
    helper(&h, &a, Provider::Claude);
    helper(&g, &h, Provider::Codex);
    helper(&k, &a, Provider::Claude);
    move_helper(&h, &b).unwrap();
    for id in [&h, &g] {
        let e = delete_by_user(id, false, Some(&a)).unwrap_err();
        assert!(e.contains(&format!("no longer under chat {a}")) && e.contains(&format!("chat \"Release\" ({b}) now")), "{e}");
        assert!(crate::background::has(id) && reg().lock().unwrap().known.contains_key(id.as_str()), "nothing was touched");
    }
    // Its chat gone first: the helper it left has no chat, and goes.
    forget(&a);
    delete_by_user(&k, false, Some(&a)).unwrap();
    assert!(!reg().lock().unwrap().known.contains_key(&k));
    // From the helper list, with no chat named: as before.
    delete_by_user(&g, false, None).unwrap();
    clean(&[&h, &b]);
}

/// A send into a helper its spawn has registered and not yet started holds
/// whose it is as a send into any helper does: the move waits for its debt and
/// closes it, and nothing of the old chat's is left owed. Both CLIs alike.
#[test]
fn a_send_into_a_helper_still_being_spawned_holds_off_a_move() {
    for (provider, tag) in [(Provider::Claude, "cl"), (Provider::Codex, "cx")] {
        let [a, b, h] = ["a", "b", "h"].map(|s| format!("sp-{tag}-{s}"));
        chat(&a, true, "bypassPermissions");
        chat(&b, true, "bypassPermissions");
        registered(&h, &a, provider);
        let stop = stop_at("send checked", &a);
        let sending = {
            let (a, h) = (a.clone(), h.clone());
            std::thread::spawn(move || crate::agents::send(&a, &h, "one more thing", false, None, true))
        };
        stop.wait();
        // Its spawn goes on: its background record is made, its first turn on.
        crate::background::idle_for_test(&h, provider, 2, false);
        crate::background::set_parent_for_test(&h, Some(a.clone()));
        crate::background::in_turn_for_test(&h);
        let mover = {
            let (h, b) = (h.clone(), b.clone());
            std::thread::spawn(move || move_helper(&h, &b))
        };
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(!mover.is_finished(), "the move waits for the send");
        stop.go();
        assert!(sending.join().unwrap().unwrap().queued);
        assert!(mover.join().unwrap().unwrap().owed);
        assert!(!owes(&a, &h) && owes(&b, &h), "the send's debt went with the move");
        clean(&[&h, &a, &b]);
    }
}

/// An answer into a helper its spawn has registered and not yet started holds
/// whose it is as an answer into any helper does: the move waits for its debt
/// and closes it. Claude Code's ask, held through `luna_permission`; a Codex
/// helper's asks come as its server's requests (background/appserver.rs),
/// held the same way.
#[test]
fn an_answer_into_a_helper_still_being_spawned_holds_off_a_move() {
    let [a, b, h] = ["a", "b", "h"].map(|s| format!("sa-{s}"));
    chat(&a, true, "bypassPermissions");
    chat(&b, true, "bypassPermissions");
    registered(&h, &a, Provider::Claude);
    let stop = stop_at("answer checked", &a);
    let answering = {
        let (a, h) = (a.clone(), h.clone());
        std::thread::spawn(move || crate::agents::answer(&a, &h, None, &["Blue".to_string()], None, None, false))
    };
    stop.wait();
    // Its spawn goes on: its first turn on, a question held by now.
    crate::background::idle_for_test(&h, Provider::Claude, 2, false);
    crate::background::set_parent_for_test(&h, Some(a.clone()));
    crate::background::in_turn_for_test(&h);
    crate::background::hold_question_for_test(&h);
    let mover = {
        let (h, b) = (h.clone(), b.clone());
        std::thread::spawn(move || move_helper(&h, &b))
    };
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(!mover.is_finished(), "the move waits for the answer");
    stop.go();
    assert!(answering.join().unwrap().is_ok(), "it went in before the move");
    assert!(mover.join().unwrap().unwrap().owed);
    assert!(!owes(&a, &h) && owes(&b, &h), "the answer's debt went with the move");
    clean(&[&h, &a, &b]);
}

/// The old chat's stop that passed its check before the move is made before
/// the move goes on; one after it is refused naming the new chat, and leaves
/// the turn the new chat has alone.
#[test]
fn a_move_racing_a_stop_leaves_the_new_chats_turn_alone() {
    let [a, b, h] = ["a", "b", "h"].map(|s| format!("rk-{s}"));
    chat(&a, true, "bypassPermissions");
    chat(&b, true, "bypassPermissions");
    helper(&h, &a, Provider::Claude);
    crate::background::in_turn_for_test(&h);
    let stop = stop_at("stop checked", &a);
    let stopping = {
        let (a, h) = (a.clone(), h.clone());
        std::thread::spawn(move || crate::agents::kill(&a, &h))
    };
    stop.wait();
    let mover = {
        let (h, b) = (h.clone(), b.clone());
        std::thread::spawn(move || move_helper(&h, &b))
    };
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(!mover.is_finished(), "the move waits for the stop");
    assert_eq!(parent_of(&h).as_deref(), Some(a.as_str()));
    stop.go();
    assert!(stopping.join().unwrap().is_ok(), "stopped before the move");
    mover.join().unwrap().unwrap();
    // The new chat's turn: the old chat's stop is refused, and stops nothing.
    crate::background::gone_for_test(&h);
    crate::background::idle_for_test(&h, Provider::Claude, 3, false);
    crate::background::in_turn_for_test(&h);
    let e = crate::agents::kill(&a, &h).err().unwrap();
    assert!(e.contains("the person moved") && e.contains(&b), "{e}");
    assert_eq!(crate::background::work_and_count(&h), Some((true, 3)), "its turn runs on");
    clean(&[&h, &a, &b]);
}

/// A wait that took its answer as the move came puts back no debt of the old
/// chat's for what the lead says once its own helpers answer: the move closed
/// what that chat was owed. Without a move, the same wait puts it back.
#[test]
fn a_wait_ending_as_the_move_comes_puts_back_no_debt_of_the_old_chats() {
    use crate::agents::owed::shown;
    let [a, b, h, g] = ["a", "b", "h", "g"].map(|s| format!("wd-{s}"));
    chat(&a, true, "bypassPermissions");
    chat(&b, true, "bypassPermissions");
    helper(&h, &a, Provider::Claude);
    helper(&g, &h, Provider::Codex);
    owe_from(&a, &h, 2, 0);
    owe_from(&h, &g, 2, 0);
    let stop = stop_at("owed on", &a);
    let waiting = {
        let (a, h) = (a.clone(), h.clone());
        std::thread::spawn(move || shown(&a, &h, 3))
    };
    stop.wait();
    move_helper(&h, &b).unwrap();
    stop.go();
    waiting.join().unwrap();
    assert!(!owes(&a, &h) && !owes(&a, &g), "nothing owed to the old chat");
    owe_from(&b, &h, 3, 0);
    shown(&b, &h, 4);
    assert!(owes(&b, &h), "still owed what it says once its own helper answers");
    clean(&[&g, &h, &a, &b]);
}

/// A helper moved and then opened by the person as a chat of their own is a
/// chat to its former chat like any other (R1), not a helper moved away; one
/// whose new chat has gone since hangs from no chat, and its former chat is
/// told that, and offered it, while its next turn still hears of the move.
#[test]
fn a_move_is_not_told_of_once_its_helper_is_a_chat_or_its_new_chat_is_gone() {
    let [a, b, h, k] = ["a", "b", "h", "k"].map(|s| format!("ms-{s}"));
    chat(&a, true, "bypassPermissions");
    chat(&b, true, "bypassPermissions");
    helper(&h, &a, Provider::Claude);
    helper(&k, &a, Provider::Codex);
    move_helper(&h, &b).unwrap();
    move_helper(&k, &b).unwrap();
    assert!(authorize_send(&b, &h).is_ok());
    assert!(authorize_send(&a, &h).err().unwrap().contains("the person moved"));
    crate::agents::take_over(&h, || None).unwrap();
    assert!(!reg().lock().unwrap().moved.contains_key(&h), "a chat now, moved as a helper no more");
    assert!(authorize_send(&a, &h).is_ok(), "chat to chat");
    for verb in ["send to", "answer"] {
        assert_eq!(in_scope(&a, &h, Some("all"), verb), Ok(()), "{verb}");
    }
    // The chat it was taken from (its new chat, B) writes into it as into any
    // chat, with scope "all": ahead of what it was taken from (`write_reach`).
    assert_eq!(reg().lock().unwrap().taken_over.get(&h), Some(&b));
    assert!(authorize_send(&b, &h).is_ok(), "chat to chat");
    for verb in ["send to", "answer"] {
        assert_eq!(in_scope(&b, &h, Some("all"), verb), Ok(()), "{verb}");
        assert!(in_scope(&b, &h, None, verb).err().unwrap().contains("the person took"), "{verb}: its own no more");
    }
    forget(&b);
    let e = authorize_send(&a, &k).err().unwrap();
    assert!(e.contains("has no chat above it") && !e.contains("the person moved"), "{e}");
    assert_eq!(ownership_of(&k).suggested.into_iter().map(|s| s.chat).collect::<Vec<_>>(), std::slice::from_ref(&a));
    assert!(move_note(&k).unwrap().text.contains("which has gone since"), "its next turn still hears of it");
    clean(&[&k, &h, &a]);
}

/// A line that grew with no chat above it does not move under a chat deeper
/// than a spawn could have made it there: a lead left alone, which spawned a
/// helper with tools, would put that one a helper's helper with tools. With
/// that one gone, it fits.
#[test]
fn a_move_is_refused_where_its_line_would_go_deeper_than_a_spawn() {
    let [gone, b, lead, sub, low] = ["gone", "b", "lead", "sub", "low"].map(|s| format!("dp-{s}"));
    chat(&gone, true, "bypassPermissions");
    chat(&b, true, "bypassPermissions");
    let tools = |id: &str| reg().lock().unwrap().known.get_mut(id).unwrap().tools = true;
    helper(&lead, &gone, Provider::Claude);
    tools(&lead);
    forget(&gone);
    helper(&sub, &lead, Provider::Claude);
    tools(&sub);
    helper(&low, &sub, Provider::Codex);
    let e = move_helper(&lead, &b).unwrap_err();
    assert!(e.contains(&format!("{sub}, a helper with Luna's tools, would be 2 levels below chat {b}")), "{e}");
    assert!(e.contains("deeper than a spawn could put it there"), "{e}");
    assert_eq!(parent_of(&lead), None, "nothing moved");
    let menu = move_targets(&lead);
    assert!(menu.targets.iter().any(|t| t.id == b && !t.ok && t.why.as_ref().is_some_and(|w| w.contains("deeper"))));
    // Its helper gone, the one under it passes up to the lead: two levels, no tools.
    crate::background::gone_for_test(&sub);
    forget(&sub);
    assert_eq!(move_helper(&lead, &b).unwrap().sessions, [lead.clone(), low.clone()]);
    clean(&[&low, &lead, &b]);
}

/// A spawn that passed its depth check before its caller's line was moved
/// under a chat, and registers its helper after (it waited on the worktree
/// queue), is asked again where that helper would sit: a lead left alone may
/// spawn a helper with tools, but not once it is a chat's helper itself. One
/// without tools still fits there.
#[test]
fn a_spawn_under_way_as_its_line_moves_goes_no_deeper_than_a_spawn_made_now() {
    let [gone, b, lead, sub, low] = ["gone", "b", "lead", "sub", "low"].map(|s| format!("sd-{s}"));
    chat(&gone, true, "bypassPermissions");
    chat(&b, true, "bypassPermissions");
    helper(&lead, &gone, Provider::Claude);
    reg().lock().unwrap().known.get_mut(&lead).unwrap().tools = true;
    forget(&gone);
    assert_eq!(parent_of(&lead), None, "a lead with no chat above it, at the top of its line");
    move_helper(&lead, &b).unwrap();
    let e = spawned(&sub, &lead, Provider::Claude, true).unwrap_err();
    assert!(e.contains(&format!("the person moved {lead}'s line under a chat")), "{e}");
    assert!(e.contains("would now be 2 levels below its chat") && e.contains("spawn it without tools"), "{e}");
    assert!(!crate::agents::registry::is_known(&sub), "not registered");
    spawned(&low, &lead, Provider::Codex, false).unwrap();
    assert_eq!(parent_of(&low).as_deref(), Some(lead.as_str()));
    clean(&[&low, &lead, &b]);
}

/// A move refused once it held the helpers leaves a turn claimed meanwhile,
/// whose launch has not seen the move yet, to start as it was claimed:
/// nothing is put back in the queue. A launch that saw the move first
/// (`overtaken`) has put its turn back by then, and it starts again under the
/// same parent once the move lets go: that one bounces.
#[test]
fn a_refused_move_leaves_a_claimed_turn_not_yet_launched_to_start() {
    let [a, plain, h] = ["a", "plain", "h"].map(|s| format!("rf-{s}"));
    chat(&a, true, "bypassPermissions");
    chat(&plain, false, "bypassPermissions");
    helper(&h, &a, Provider::Claude);
    crate::background::in_turn_for_test(&h);
    assert_eq!(crate::background::send(&h, "plain", Some(&a)).unwrap(), crate::background::Placed::Queued);
    crate::background::reserved_for_test(&h);
    assert!(move_helper(&h, &plain).unwrap_err().contains("without Luna's tools"));
    assert!(crate::background::launch_for_test(&h), "its turn starts as claimed");
    assert!(crate::background::queued_for_test(&h).is_empty(), "nothing put back");
    crate::background::launch_failed_for_test(&h);
    crate::background::gone_for_test(&h);
    clean(&[&h, &a, &plain]);
}
