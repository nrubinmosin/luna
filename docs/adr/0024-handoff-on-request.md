# ADR-0024: List the handoff to every chat with Luna's tools, and answer it only when asked

- **Status:** Accepted; amends
  [ADR-0020](0020-session-self-handoff.md) (opt-in),
  [ADR-0022](0022-content-luna-serves-at-its-own-trigger.md) (who knows) and
  [ADR-0023](0023-moving-a-chat-to-another-account.md) (who writes a move's
  document)
- **Date:** 2026-11-12

## Context

[ADR-0020](0020-session-self-handoff.md) made the handoff opt-in: only a
person's chat with Auto-handoff ticked was listed `luna_session_handoff`, and
only its threshold asked it.
[ADR-0022](0022-content-luna-serves-at-its-own-trigger.md) kept every word of
it from any other session. In use that fell short in three ways:

- The owner wants a handoff when the work reaches a point, not when the
  context reaches a share. A chat with no threshold could not be asked at
  all, and one with a threshold only by it.
- Ticking Auto-handoff on a running chat cost a quick resume, as a CLI lists
  tools only as it starts and Luna's server cannot announce a new one.
- A move ([ADR-0023](0023-moving-a-chat-to-another-account.md)) could ask the
  chat for its document only with auto-handoff on. Otherwise, and whenever an
  ask failed, the move fell back to the document Luna builds from records,
  without a word. That document is a mechanical cut of the transcript; the
  chat's own is the better one.

The tool is destructive to the chat's own conversation: a call with `file`
clears it. Listed to more chats, it is in reach of more prompt injection, so
listing it is not to mean answering it.

## Decision

The handoff is listed by who the chat is and answered by who asked
(`agents/handoff/mod.rs`, `store.rs`, `moving.rs`):

- **Listed.** Every person's chat with Luna's tools is listed
  `luna_session_handoff` (`may_list`: tools, no parent, not an orphan, a
  helper or the dev caller), auto-handoff on or not, and gets its drafts
  folder at every start: Claude Code with its folder of copies, Codex under a
  sandbox that writes at all. To every other session it is still a tool there
  is not.
- **Answered only under an open ask** (`Record::asked`, kept in
  `handoffs.json`). An ask is the threshold's (`AskKind::Auto`, opened with its
  notice), the person's (`AskKind::Manual`, from the pane's **Hand off now**,
  the core command `handoff_now`), or a move's (`MovePhase::Asking`/`Asked`).
  Unasked, the call with or without `file` is refused, naming the pane's menu
  (`NOT_ASKED`).
- **An ask is bound to its conversation** (`Asked::conversation`): it holds
  only there (`ask_holds`), and the beat closes it once the chat is on
  another, as after the person's `/clear` or `/new` (`drifted`). The
  threshold's ask closes as the threshold re-arms. The person's lapses ten
  minutes after Luna typed it (`MANUAL_ASK_MS`), unless a handoff answering it
  is scheduled. Turning auto-handoff off closes only the threshold's ask. A
  record from before asks were kept is re-armed at load, so its threshold asks
  it again.
- **The person's request is typed strictly**, as a move's ask is: at a strict
  prompt, the turn state looked at again with the person's keys held
  (`type_manual`), not through the notifier, which could paste into a Codex
  turn just begun. A CLI started before the tool was listed to it gets a
  quick resume on the same conversation on the first press
  (`HandoffNow::resume`).
- **Moves are written by the chat by default.** Any chat with Luna's tools
  whose CLI listed the tool may be asked (`ask_blocked` says what stands in
  the way and how to clear it). Luna's records-built document is taken only by
  the person's choice: before the ask, during it ("Use Luna's document now"),
  or after a failed one ("Build from records", `move_build_from_records`,
  which closes the ask so a late document is refused). It is preselected only
  while a usage limit holds the chat (`activity::limit_in_force`, shown with
  its window through `move_facts`): until a usage reading taken after the stop
  shows the windows of the chat's model not full, or their resets have
  passed; with no data it holds. No percentage threshold picks it. The ask's
  own turns are left out of a records-built document. A failed ask keeps why
  (`AskEnd`) and says whether asking again makes sense.
- **The move's instructions** are the handoff's in effect, the person's
  library override included, under the move's own rules
  (`served::move_instructions`).

## Alternatives weighed

- **Keep the tool hidden, quick-resume on each press.** Every Hand off now
  would restart the chat's CLI to list the tool, and another to drop it. A
  restart costs seconds at an idle moment and drops what the CLI holds in
  memory; and a hidden tool still had to be answered only when asked, so the
  gate was owed either way. Rejected.
- **Remove the records-built document.** It is the only one to be had from a
  chat that is stopped on a limit, not running, or cannot write a file.
  Kept, by explicit choice.
- **Let the new account's chat read the old transcript.** The transcript
  lives in the old account's folder, beside its credentials; giving another
  account's CLI a read there crosses the account-folder rules and sends
  whatever the transcript holds without the person's preview (threat model
  §3, the move's bullets; B5). Rejected.

## Consequences

- Every person's chat with Luna's tools pays for one more tool description in
  its context, whether it hands off or not.
- Every such Codex chat under a sandbox that writes gets its drafts folder as
  an `--add-dir`, and on Windows Codex's unelevated sandbox grants itself an
  inheritable Modify ACE there (threat model B4, the drafts row). Before, only
  opted-in chats had it.
- An injected prompt in a tooled chat can call the tool, but an unasked call
  does nothing; under an open ask it can at most hand off the conversation the
  ask names, which the person or the threshold asked for anyway. It cannot
  open an ask: only the window, the threshold's beat and a move do.
- Auto-handoff is now only a trigger. Its notices still go through the
  notifier; the person's request and a move's ask do not.
- A failed move ask no longer turns into Luna's document unseen; the person
  picks Build from records, Ask again or Cancel.
- What ADR-0022 says of who knows changes: every person's chat with Luna's
  tools knows of the tool; helpers, the dev caller and sessions without tools
  still know nothing of it.
