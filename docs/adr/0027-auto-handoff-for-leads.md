# ADR-0027: Hand a lead off to a fresh conversation of its own at its turn's end

- **Status:** Accepted; amends
  [ADR-0024](0024-handoff-on-request.md) (the tool is listed to leads too)
- **Date:** 2026-11-13

## Context

A lead, a helper spawned with Luna's tools
([ADR-0026](0026-session-roles-at-memory-level.md)), runs long campaigns, and
its delegates' notices fill its context until its CLI compacts it, which keeps
far less than a handoff. A person's chat with Luna tools already goes on in a
fresh conversation from a handoff it writes when asked
([ADR-0020](0020-session-self-handoff.md),
[ADR-0024](0024-handoff-on-request.md)), but that switch is made of a
terminal: `/clear` or `/new` typed, the new conversation read off the CLI's
registry or title. A lead has no terminal: each turn is one `claude -p` or
`codex exec` run, resuming the conversation the first turn named.

What was on the table: leaving leads to their orchestrator (it cannot see
their context, and a fresh helper loses the lead's id, helpers and debts);
asking the lead to spawn its own successor (same loss, and a helper of a
helper's places); or the chats' mechanism carried over to the headless runner.

## Decision

While **Auto-handoff for leads** is on (`Settings::lead_handoff`, on at 60% by
default), `luna_session_handoff` is listed to every lead
(`agents/handoff/mod.rs` `may_list`, `lead.rs` `listed_to`) and to no helper
without Luna's tools. At the end of each of its turns, at rest, its context is
held to that threshold as a chat's is (`watch.rs`); past it Luna opens the
threshold's ask, bound to the conversation whose context passed it, and queues
it as a turn of Luna's own (`background/switch.rs` `send_luna`), past the firm
mark ahead of anything queued. A lead that writes no file (Claude Code without
its edit tools or in plan mode, Codex in a read-only sandbox) is not asked:
Luna writes its handoff from its records (`document.rs`, as for a move).

The call with `file` is checked and copied as a chat's (`file.rs`) and
schedules the switch on the conversation that called; from then on what is
sent to the lead queues and never joins that turn on stdin. At that turn's
end the switch is the next turn: no `--resume`, no `resume`, Luna's opening
(the copy, the skills to load first, the approvals' bound, `fact_block`) on
stdin, in the worktree its first turn made and without `--worktree`, which
would make another. The conversation its CLI names in that turn's `init` or
`thread.started` is the new one. The lead's id, token, role layers,
constraints, helpers and their debts stay: they are keyed by its id, and
every turn's command rebuilds the rest. Its parent hears of it as a line
riding its next notice.

The switch is kept with the helper, `LeadSwitch { n, copy, phase: Scheduled |
Fresh | Done, … }` in `helpers.json`, not in `handoffs.json`: it changes in the
same moves as the helper's conversation and queue, and one write keeps the
three agreeing across a core's death. The asks, what each conversation was
told and the handoffs through or given up are in `handoffs.json` under the
lead's id, as a chat's.

## Consequences

- A lead keeps its campaign going past its context without its orchestrator
  doing anything, and a stuck switch costs nothing: one that cannot start (what
  its turns left still running, a stopped turn, a copy that is not Luna's, a
  fresh start that names no conversation) goes back to the conversation it
  left, which nothing changed, and the lead is told.
- `luna_session_handoff` is no longer a person's chat's alone. Its refusals
  hold as before: unasked it does nothing, and an ask holds only in its
  conversation. Luna's default `role-lead` text names it to every lead, the
  setting on or off.
- Codex's `exec resume` takes no `--add-dir` (0.167.2), so a Codex lead is
  given no drafts folder: it writes its handoff in its workspace, outside
  Luna's folder, as the file check already allows.
- A core started again goes on with a switch whatever `resumeOnRestart`
  says, and gives it up where nothing says the last core's turn ended.
- `background_promote` is refused while a switch is under way.
- Owed: the live checks of the whole path on both CLIs.
