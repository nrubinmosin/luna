# ADR-0016: Helpers run in the background only; opening one hands it to the person

- **Status:** Accepted
- **Date:** 2026-11-09

## Context

`luna_spawn` made a helper one of two ways: in the background, a headless CLI
run per turn in the core (the default), or with `visible: true` as a chat row
in the window, a CLI in a terminal under the spawning chat. "Open as chat" also
turned a background helper into such a row, still its agent's.

A visible helper was its agent's and a terminal at once, and most of the agent
layer existed to make that hold: a round trip to the page for every spawn
(`agent://spawn`, `agent_spawned`, requests kept for a page that was not
listening), carrying one on after a core restart (`resumeOnRestart` for chats,
openings kept until a first turn ended, worktrees named so a fresh start could
go back into them), raw `keys` and permission presses on its screen, Esc to
interrupt it, text held for a draft in it and typed mid-turn, its row reparented
when its parent went, its lineage kept across cores in `chat-ties.json`, and a
rule that one a person typed into was never deleted. A cost study put this at
about 3,000 lines, and found it fragile: two parties (the agent and the person)
could type into one terminal, and every rule above existed to keep them from
colliding.

What a visible helper gave that a background one did not was a person watching
it work, and steering it. Watching needs only its transcript, which the core
already reads for `luna_read`. Steering a turn in flight is possible only for
Claude Code (Codex's `exec` reads its input once).

The options were: drop `visible` and keep "Open as chat" with the agent still
attached (option A), or drop `visible` and make "Open as chat" hand the helper
to the person for good (option B).

## Decision

Option B, the owner's decision of 2026-11-08.

- Every helper runs in the background (`agents/spawn/`, `background/`).
  `luna_spawn` has no `visible`, and a Codex helper's `approval` is `never` or
  `auto-review`: one that asks a person (`on-request` and the like) is refused.
  An old caller passing `visible` gets the standard unknown-argument error.
- A chat the window runs is the person's and never anyone's descendant:
  `ensure_*_session` registers it with no parent, starts it with no opening
  prompt, helper note or helper settings, and no agent interrupts, stops,
  deletes or approves for it. `luna_answer` loses `keys`.
- "Open as chat" hands the helper over for good (`agents/handover.rs`). While it
  is being opened (`promoting`) no turn starts and its parent can neither stop
  nor delete it. Once the row exists, one registry hold takes its parent and
  every debt anyone was owed by it; then its record goes with what was queued
  for it, which is never delivered; then each session affected hears once, in
  a line saved in `chat-ties.json`. Its former parent's calls are refused with
  the takeover's words, and a `wait` it has out ends.
- The person watches a helper read-only (`background_transcript`, by the
  readers `luna_read` uses).
- The person's Delete never stops a working helper, or drops a worktree with
  unsaved work, on a parent chat's checkbox alone (`delete_by_user`).

## Consequences

- The agent layer loses the window round trip, the chat carry-on, the screen
  answers, the interrupt by Esc, the reparenting and the visible helpers' lineage
  across cores. What stays for terminals is what the person's own chats need:
  notices to a chat with tools, `luna_send` with `scope: "all"` into a chat,
  its draft put aside or the text held, and answering a question a chat's model
  asked.
- An agent loses a helper the moment the person opens it, and cannot steer it
  together with the person. A person who wants to steer and let the agent carry
  on talks to the agent instead.
- A Codex turn cannot be steered by anyone mid-turn; a Claude Code one only by
  its agent (`luna_send` joins at the next tool boundary).
- A chat an earlier Luna ran as a visible helper comes back as the person's
  own, its row no longer nested.
- `luna_spawn`'s answer loses `started`, `screen`, `ask`, `starting` and
  `warnings`, which only a visible start could fill; `resolvedModel` in
  `luna_list` says which model ran.
