# ADR-0023: Move a chat to another account of its CLI by a document the person confirms

- **Status:** Accepted; who writes the document amended by
  [ADR-0024](0024-handoff-on-request.md)
- **Date:** 2026-11-11

## Context

A chat's account runs out (a usage window near its end, or a turn cut off by
the limit) while the work in it is far from done. Its conversation cannot
follow it: a Claude Code transcript and a Codex rollout live in the account's
own folder, beside its credentials, and neither CLI resumes another account's
conversation. What can go over is a document and the working folder.

What was on the table, and what the live checks on Claude Code 2.1.322 and
Codex 0.167.1 found (docs/cli-compatibility.md):

- **Across CLIs.** Permission modes do not map: Claude Code's `readOnly` keeps
  Bash, which writes, so a Codex read-only sandbox has no equal there, and
  Codex approvals have none in Claude Code. A move that is "no wider than the
  source" cannot be had across them.
- **Worktrees.** A Claude Code worktree lives in the repository, and another
  account can work in it; but the session that made it removes it as it
  quits when it is clean, even with another session inside, leaving a folder
  whose `.git` points nowhere. A Codex worktree lives in the account's own
  folder (`CODEX_HOME/worktrees`), beside its `auth.json`.
- **Who writes the document.** The chat itself knows its work best, but asking
  it is typing into it: a Codex line typed mid-turn steers the turn (cuts the
  answer short), plan mode and read-only sandboxes write no file, and Claude
  Code's default mode asks the person before writing. Luna can write one
  itself from the chat's records, with no model call, at any time, from a
  chat that is stalled or not running.
- **First starts.** An account that never ran the CLI shows first-run screens
  (Claude Code: theme, security notes, a trust screen whose default is "No,
  exit"; Codex: its trust screen), which a line typed on a timer would land in.
- **Egress.** The document goes to another account, possibly another
  organization: what it carries must be what the person saw.

## Decision

1. **Same CLI only.** Claude Code to Claude Code, Codex to Codex. A Codex chat
   in a worktree does not move. A Claude Code chat in a worktree moves into
   the same worktree by name (`--worktree <name>`), and its old chat is always
   stopped before the new one starts: the window stops it, the core looks
   (`move_step`), and a start of the new chat is refused while the old one runs
   (`opening_refusal`, pty/start.rs).
2. **Two sources of the document.** Luna writes it from the chat's records
   (agents/handoff/document.rs): rules of going on, the person's note, the
   state (git with its time, how the chat stopped, the files its edit tools
   were called on), the task (its last handoff, else its first prompt and its
   last compaction summary), the plan it last proposed, turns of the
   conversation chosen newest first within a budget, its helpers, first
   steps; read bounded at both ends of the transcript (excerpts.rs), git with
   its own limits (gitfacts.rs). Or, for a chat with auto-handoff on, the chat
   writes it through its own `luna_session_handoff`, served a text of its own
   for a move and collected without a reset; only at strict idle, never in
   plan mode, read-only, stalled or not running. That is a second text Luna
   serves at its own trigger, to the chat that opted in alone, as
   [ADR-0022](0022-content-luna-serves-at-its-own-trigger.md) asks of any.
   (Amended 2026-11-12 by [ADR-0024](0024-handoff-on-request.md): any chat
   with Luna's tools whose CLI listed the tool is asked by default; Luna's
   document only by the person's choice, preselected only while a usage
   limit holds the chat, and a failed ask says why instead of falling back.)
3. **A mandatory, editable preview of everything that goes.** The line Luna
   types and the whole document are shown; the document may be edited. Send
   is bound by one hash to the text, the account it goes to and the launch
   settings (`binding`), which also hold the new chat's start
   (`start_mismatch`): another account, folder or permissions, and nothing is
   typed. Folders of accounts and of Luna's are cut out before the preview and
   refused after an edit; secret-shaped strings are taken out, best effort,
   and counted; long random strings are highlighted, not removed. The preview
   is consent, not a certificate that the text is clean.
4. **Durable phases.** Shown, Confirmed, OldStopped, RowCreated, Prompting,
   Done (Failed, Canceled), each on disk on the old chat's handoff record
   before what follows; each taken again safely. A preview a restart finds
   unconfirmed is given up, never sent. The new chat's line is typed only at a
   strict prompt (the composer drawn and empty, its turn not under way, nobody
   typing), with no wait shorter than a day while its CLI runs: a person may
   have first-run screens to pass in the pane.
5. **What does not move.** The conversation, the MCP token (a new chat, a new
   one), what is owed, held sends, the CLI's own jobs, the account's memory,
   connectors and instructions. Helpers stay with the old chat; the document
   names them as its, and stopping the old chat with live helpers is warned of.
   Reparenting helpers is a decision of its own, not taken here.

## Consequences

- No MCP tool starts a move: only the person's window does, so no agent can
  send a chat's history to another account.
- The core grows five page commands (`move_prepare`, `move_confirm`,
  `move_step`, `move_cancel`, `move_list`) taking chat ids, an account path
  checked as `AccountPath`, and a text the person confirmed; threat model B1
  and B2.
- What Luna types into a chat grows by one fixed line pointing at the
  document (`[Luna move <id>, not typed by the user]`, threat model B4); a
  moved chat's Claude Code start gets its folder of copies as an `--add-dir`
  (`moved-chat` in What Luna applied).
- The document quotes the earlier conversation: model input, fenced by a
  random boundary per document, a line opening `[Luna ` escaped, tool output
  never included (R2). What a person pasted into the chat themselves is the
  residual leak.
- Two chats in one folder are possible when the old one is kept; the dialog
  warns of it. Deleting either chat of a moved Claude Code worktree keeps the
  worktree the other uses.
- Owed: a live suite case per CLI on throwaway accounts (docs/testing.md),
  and the fragile parts of the document (plans, compaction summaries, edit
  paths) checked again with each CLI upgrade (the `cli-upgrade` skill).
