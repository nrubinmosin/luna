# ADR-0019: Hand a chat over to a fresh conversation in place, on its own word

- **Status:** Accepted
- **Date:** 2026-11-09

## Context

An orchestrating chat that runs a long campaign fills its context window with
coordination: helper answers, notices, decisions. Past a point every call costs
the whole context again, and the CLI's own compaction keeps a summary it writes
itself, which loses what an orchestrator owes (which helper was asked what,
branches and SHAs, decisions made with the person). The owner's practice was to
have the chat write a handoff file and start a new chat by hand; that broke the
helper tree, as helpers, their debts and notices stay keyed to the old chat id.

Options weighed (two analyses, 2026-11-08): a successor session in the same row
or a new one beside it, both needing helpers, debts, waits and notice state moved
to a new owner; CLI hooks (`SessionStart` with `clear` or `compact`), which can
re-inject a file but cannot make the model write one; a fresh CLI process behind
the same chat id, generation-fenced; or the CLI's own reset typed into the chat:
Claude Code's `/clear`, Codex's `/new`. A live validation (stage 0, Claude Code
2.1.319, Codex 0.167.0) found the reset keeps everything the switch needs: the
same process, Luna's MCP connection (Codex re-initializes it with the same
token), Luna's instructions and hooks, and Luna already follows a chat onto its
new transcript (`read.rs` `moved_to`). Luna's MCP server cannot announce a new
tool to a running CLI (no server-to-client stream; Codex's `list_changed` is off
by default). The owner's constraint: never for every session, only one the person
opts in, with a threshold.

## Decision

Reset in place, opt-in per chat, on the session's own call (`agents/handoff/`):

- **Opt-in.** A person's chat with Luna tools gets "Auto-handoff at N%" in the
  new-chat dialog or its What Luna applied panel, kept on the chat and passed at
  every start (`handoff` in its registry settings). Only such a chat is offered
  `luna_session_handoff` in `tools/list`; the tool refuses any other caller.
  Turned on while the chat runs, the window resumes its CLI on the same
  conversation once it is idle, so the CLI lists the tool.
- **Trigger.** At each turn's end Luna reads the chat's context (`Context::of`);
  past the threshold one notice, with the procedure and the handoff's sections,
  and a firmer one near where the CLI would compact; once each per
  conversation. Luna never switches by itself.
- **The call.** `luna_session_handoff({file})` takes no target: it acts on the
  chat its token is. Refused while the CLI's own jobs or subagents run (named).
  The file is checked (Markdown or text, absolute, no link, at most 128 KB, UTF-8,
  not inside Luna's data folder) and copied to `<data>/handoffs/<chat>/<n>.md`
  with its SHA-256.
- **The switch.** A machine whose phases are saved (`handoffs.json`): Scheduled
  (notices to the chat and text sent to it wait) → at rest on two looks with no
  draft → Resetting (Luna types `/clear` or `/new`, fixed text) → the CLI on
  another conversation (Claude Code's registry session id, Codex's thread in its
  title) → Prompting (Luna types `[Luna handoff N` with the copy's path and a fact
  block it writes itself: helpers with task, state, branch, SHA and worktree,
  answers owed, text held, accounts and their use, the chat's settings) → the new
  transcript holds the prompt → done, and what waited goes in. A core that stops
  mid-switch leaves it Recovering: once the chat is back, the old conversation
  means try again (twice at most), a new one means type the prompt if it is not
  in. A failure before the reset tells the old conversation and leaves it as it
  was; one after tells the new one what it is and where to start.
- **Window.** One row; a divider over the terminal: "Handoff N · time · open
  handoff · previous conversation", the latter in the read-only transcript view.
  Codex's chat name is pinned (a conversation's first prompt that is Luna's is
  no name).

## Consequences

- The chat id, token, helpers, debts, waits and notice state never move: no
  reparenting or owner transfer exists, and none is needed.
- The CLI's own background jobs and subagents cannot cross; the session must end
  them first. Luna helpers cross untouched.
- The switch rests on two CLI behaviors Luna does not control: what `/clear` and
  `/new` keep (docs/cli-compatibility.md) and how each CLI says it is on a new
  conversation. A CLI that changes either fails the switch before or after the
  reset, never silently; `cli-upgrade` checks both.
- The handoff is still a summary a model writes. Luna makes it deliberate and
  adds the facts that go stale in a file, but cannot make it complete.
- Turning auto-handoff on for a running chat restarts its CLI once (same
  conversation): a few seconds, at a moment the chat is idle.
- New attack surface, kept to the caller's own chat: a tool with no target, a
  file copied from a path the session names (checked against Luna's folder and
  links), and fixed text typed into the person's terminal (threat model §5).
