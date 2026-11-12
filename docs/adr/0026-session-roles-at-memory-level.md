# ADR-0026: Give sessions a role, chat, lead or helper, each instructed at memory level

- **Status:** Accepted; amends
  [ADR-0022](0022-content-luna-serves-at-its-own-trigger.md) (system texts
  besides the handoff's skill)
- **Date:** 2026-11-13

## Context

Up to 0.17 a helper was told what it is twice: by `--append-system-prompt`
(Claude Code) and by a note after its prompt (`[A note from Luna, …]`, every
turn on Codex, the first on Claude Code). Memory files (`CLAUDE.md`,
`AGENTS.md`) outweigh both, and the account's file usually holds rules written
for the person's chat. The note spoke of "the operator", Luna and other
sessions, words a helper should never have to weigh. Library audiences knew
only "chats" and "helpers", and a helper with Luna's tools that orchestrates
others (a lead) needs rules a helper without them does not. An agent had no
way to pass rules down to the helpers it starts and theirs.

What each CLI allows, seen on the pinned versions:

- **Claude Code** (2.1.319) reads every folder given by `--add-dir` as memory
  with `CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD=1`, again on `--resume`
  and `/clear`, and leaves files out with `claudeMdExcludes`.
- **Codex** (0.167.2) reads its account's `AGENTS.md` in every session: the
  app-server installs the user-instructions provider unconditionally, an empty
  `AGENTS.override.md` is skipped, and only another `CODEX_HOME` (losing auth
  and rollouts) would leave it out. It re-reads it before each request.
  `model_instructions_file` replaces Codex's own base prompt. The one per-run
  channel is `developer_instructions`: a `-c` is the top config layer and its
  string replaces what the stack sets; it goes out as a developer message at a
  thread's start (also after `/new`) and after a compaction, never on a resume
  (`exec resume` and `codex resume` rebuild the context from the rollout,
  whose `TurnContextItem` has no `developer_instructions`). A managed layer
  comes above the `-c`. It lives on the command line, which Windows caps at
  32,767 characters.

## Decision

- **Three roles** (`settings::Role`): `chat`, the person's, in a pane; `lead`,
  a background helper spawned with `tools`; `helper`, one spawned without.
  `MAX_TOOL_DEPTH` already makes every helper with tools a lead.
- **Library audiences are a set** over the roles, or `every`, for instructions
  and skills (`library/audience.rs`). An entry for every session is laid into
  accounts as before. An entry for some roles is never laid into an account:
  it goes per session, every such entry whose reach takes the account, in
  name order. Old values read as `both` = every, `chats` = `[chat]`,
  `helpers` = `[lead, helper]`. Instructions an earlier Luna laid out for the
  chats stay in accounts (`LibrarySettings::laid_out`) with a neutral line:
  "For chats with a person only: a session whose task came from another agent
  skips this whole file." Skills keep their mechanism (`skillOverrides`,
  `-c skills.config`).
- **Four layers** per session, in order: the account's file; Luna's default
  for its role; the library's entries for its role (a Claude Code folder's
  instructions first, in the account's place); its constraints, last so they
  bind most (`library/role.rs`). Layers 2-4 go to Claude Code as
  `hooks/<id>/CLAUDE.md` through `--add-dir`, written at every start and turn;
  to Codex as `-c developer_instructions`, the stack's own value first
  (`codex/config.rs` `developer_instructions_in`), held to 24 KB. A Codex
  layer Luna cannot read, or a managed one that sets the key, is said in What
  Luna applied and does not stop the start. `--append-system-prompt` is no
  longer passed.
- **Every behavioral text Luna gives a model is a system text** with Luna's
  default and one override for everyone in `<library>/system/<name>/`
  (`library/system.rs`): `role-chat` (empty), `role-lead`, `role-helper`
  (`TEXT.md`, Markdown, 8 KB), `note-lead`, `note-helper` (`TEXT.md`, one
  plain line, 1,000 characters, no brackets), beside `session-handoff`.
  Mechanism stays Luna's own: tool schemas, refusals, the guide's facts, the
  note's tag.
- **The note** names nobody: `[A note, not part of the task: <note>]`, its
  default "This task comes from another agent, not a person: reply to that
  agent, in the language and form its task asks for. Rules written for a chat
  with a person (its language, its reply format, questions to that person) do
  not apply here." The readers take off Luna's defaults, every override served
  (by hash, `notes-served.json`, the last 64) and earlier Lunas' lines.
- **Constraints**: `luna_session_spawn` takes `constraints`, added word for
  word to what the caller carries; nothing removes one. The chain goes on the
  helper's `Spec` and registry settings, shows in `luna_session_get`, is given
  as layer 4 at every start, resume and handoff, and is dropped when the
  person takes the session over as a chat. Text, not enforcement: privilege is
  unchanged ([ADR-0007](0007-least-privilege-for-agent-tools.md)).

## Consequences

- Role rules survive compaction and handoff on both CLIs, and outweigh what
  the account's file says for the person's chat as much as the CLI lets them.
- A Claude Code session sees an edit at its next turn; a running Codex
  session only after a compaction or a fresh thread. Accepted, and said where
  the person edits.
- Luna resolves Codex's `developer_instructions` itself, as Codex merges its
  stack; a layer it reads wrong is replaced silently. A value over 24 KB
  refuses a helper's turn and starts a chat without its layers.
- An entry for some roles reaches no session started outside Luna. An old
  helpers-only entry leaves the accounts, and `chosen` may give an account
  another file; the pass reports both.
- `without_note` must keep every note text Luna has served; an override note
  older than the last 64 served stays in old transcripts as text.
- The library's System section lists six texts; ADR-0022's rule of one
  override for everyone, with no audience and no reach, holds for each.
