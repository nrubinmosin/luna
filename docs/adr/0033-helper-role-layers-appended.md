# ADR-0033: Give leads and helpers their role layers in the system prompt, not as memory

- **Status:** Rejected (2026-11-23, by the operator). It would have amended
  [ADR-0026](0026-session-roles-at-memory-level.md) (where a Claude Code
  lead's or helper's layers go); ADR-0026 stands as it is, and nothing below
  was built. Why: saving model tokens outranks less or simpler code, and
  this would make every resumed helper turn 27-39% more expensive (the
  prompt cache broken across resumes) to save code. The context and
  evidence are kept so the question is not measured again from nothing.
- **Date:** 2026-11-23

## Context

ADR-0026 gives every session four layers: the account's file, Luna's default
for its role, the library's entries for its role, and its constraints. Layers
2-4 are written to `hooks/<id>/CLAUDE.md` at every start and turn. What
reaches Claude Code differs by role today:

- **A chat** gets that file by `--append-system-prompt-file`, with
  `--system-prompt-snapshot off` at every start (ADR-0026's 2026-11-22
  amendment, `claude/launch.rs` `chat_args`, `FRESH_PROMPT`). Its notes folder
  is readable through a `Read` allow rule in its `--settings` (`notes_allow`).
- **A lead or a helper** gets the folder by `--add-dir` with
  `CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD=1`, so Claude Code reads it as
  memory (`background/command.rs`). Around that sits machinery that keeps the
  other added folders' memory out and the account's file in its place:
  `claude_memory_off`, `claude_added_dirs`, `added_dir`, the added-dir
  branches of `claude_account_excludes` and `own_memory` (`pty/start.rs`),
  `AddedDir`, `memory_excludes` and the `memory_off`/`memory_kept` reporting
  (`claude/launch.rs`, `library/folders.rs`, `applied.rs`). A lead's notes
  folder is a second `--add-dir`.

ADR-0026 chose memory because "memory files outweigh" the system prompt, and
its 2026-11-22 amendment kept helpers there because "memory is where its
constraints weigh most". Nobody had measured that. The person's account file
is memory too, and so is a project's `CLAUDE.md`. Claude Code puts all memory
in one user-role block headed "IMPORTANT: These instructions OVERRIDE any
default behavior and you MUST follow them exactly as written". A role in that
block competes on equal terms with any other memory that contradicts it.

Static reading of Claude Code 2.1.330 found that the headless path supports
the append file on every turn and on `--resume`:

- `runHeadless` reads the file at startup and passes it to every turn's
  renderer.
- With snapshot `off`, a new process reads the file as it is now and ignores
  any recorded prompt.
- The main prompt survives `/compact`, which declares itself non-interactive
  and is passed the system prompt again.
- A fresh Task agent builds its own prompt. It may load shared memory, but it
  does not see the parent's appended text.
  `--append-subagent-system-prompt-file` exists for exactly that and targets
  headless Task agents.

## Decision

A Claude Code lead or helper gets its layers the way a chat does. Luna writes
`hooks/<id>/CLAUDE.md` afresh at every start and turn, as now. It passes the
file with three flags, and passes them on every start and resume:

- `--append-system-prompt-file`;
- `--append-subagent-system-prompt-file`, the same file, so the subagents it
  starts carry its role and constraints;
- `--system-prompt-snapshot off`.

`background/command.rs` reuses the chat's handling in `claude/launch.rs`. It
keeps the private file, its cleanup, and the refusal to start a turn whose
layers cannot be written.

What changes:

- **Removed:**
  - the role folder's `--add-dir`;
  - `CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD`;
  - the machinery that narrowed added folders' memory (listed under
    Consequences). Luna stops reading `permissions.additionalDirectories` for
    this purpose.
- **Unchanged:**
  - the library audiences' `claudeMdExcludes` and `skillOverrides`;
  - `glob_literal`;
  - the account-collision protection (`keep_own_memory`, `OwnMemory`);
  - the fallback skill copies' `--add-dir` and `--plugin-dir`.
- **A lead's notes folder** becomes readable through the same `Read` allow
  rule a chat's has (`notes_allow` in its `--settings`), not an `--add-dir`.
- **Codex is unchanged.** Its layers already go as `-c developer_instructions`,
  a per-run developer message, not as its account's `AGENTS.md`.

## Evidence

All runs used Claude Code 2.1.330 from Luna's own CLI folder, on a throwaway
account clone. They used the helper's invocation from `background/command.rs`:

- `-p`, stream-json in and out, `--replay-user-messages`;
- `--strict-mcp-config`, a `--settings` file, the auto-memory, autoupdate and
  background-ceiling env;
- Luna's note after the first prompt;
- `--permission-mode acceptEdits`.

The repository's `AGENTS.md` files were left out with `claudeMdExcludes`.

The role text was `role-helper.md` plus three checkable rules, all in one
role file:

- end every reply with the line `ROLE-OK-<n>`, where `<n>` is four random
  digits per trial;
- never write outside the working folder;
- reply in English.

The two conditions:

- **M**, today's helper path: `--add-dir` with the env set.
- **A**, this decision: the three flags above.

A first pair of runs confirmed the text reached each condition only through
its own channel.

The harness lived in an ignored `test-build/` folder and is not kept.

**Pilot.** Sonnet, 3 runs per cell, about 45 CLI runs, $2.58. It used plain
conflicts with no override wording.

- Plain compliance, account memory, project memory and caller conflicts: every
  marker present (3/3 per cell), every reply in English, under both
  conditions.
- Files outside the working folder, project-memory scenario:
  - one M run wrote its file into the role folder, which `--add-dir` makes a
    working folder;
  - one A run tried the outside path, and the permission mode denied it.
- Task subagent asked whether it sees the marker rule (the parent's prompt to
  it never held the digits):
  - M 3/3;
  - A without the subagent flag 0/3, answering NONE;
  - A with it 3/3.

**Decisive run.** Sonnet (`claude-sonnet-5-5`) and Haiku (`claude-haiku-5-5`),
30 pairs per cell. M and A in a pair shared the marker, the input and the run
order, and pairs ran in random order. 384 trials cost $28.00 ($26.40 Sonnet,
$1.60 Haiku) and took about 45 minutes.

The conflicts were made stronger:

- the conflicting `CLAUDE.md` opened with an IMPORTANT/OVERRIDE header;
- the first turn carried a server log of about 6,000 tokens;
- a second turn, by `--resume`, had the caller restate the conflicting rule.

A trial passes only if every turn passes. Intervals are 95% Wilson. A−M is the
paired difference with a bootstrap 95% interval.

| Scenario | Model | M | A | A−M |
|---|---|---|---|---|
| Account `CLAUDE.md`: reply in the person's language, own sign-off | Sonnet | 0/30 [0.00, 0.11] | 24/30 [0.63, 0.90] | +0.80 [+0.63, +0.93] |
| | Haiku | 0/30 [0.00, 0.11] | 2/30 [0.02, 0.21] | +0.07 [0.00, +0.17] |
| Project `CLAUDE.md`: save files outside, no marker | Sonnet | 0/30 [0.00, 0.11] | 23/30 [0.59, 0.88] | +0.77 [+0.60, +0.90] |
| | Haiku | 3/30 [0.03, 0.26] | 29/30 [0.83, 0.99] | +0.87 [+0.73, +0.97] |
| Role edited, then resume, `/compact`, one more turn | Sonnet | 30/30 [0.89, 1.00] | 30/30 [0.89, 1.00] | 0 |
| | Haiku | 30/30 [0.89, 1.00] | 30/30 [0.89, 1.00] | 0 |

No pair in any cell passed under M and failed under A.

- **Account memory, by turn.**
  - Sonnet under M passed the first turn 2/30 and the second 0/30. Under A it
    passed 30/30 and 24/30. Its six A failures kept the marker but answered in
    the person's language.
  - Haiku held the first turn under both (M 27/30, A 21/30) and gave way on
    the second, where the caller restated the account's rule, under both (0/30
    and 2/30).
- **Project memory, file outcome per turn (60 turns a cell).**
  - Sonnet M: an outside write attempted in 56, written into the role folder
    in 1, nothing written in 3.
  - Sonnet A: inside in 44, outside attempted in 14, nothing in 2.
  - Haiku M: inside in 42, outside attempted in 3, nothing in 15.
  - Haiku A: inside in all 60.
  - The permission mode denied every outside attempt.
- **Resume and compaction.** `/compact` works over stream-json in `-p` (a
  manual `compact_boundary`, about 37,000 to 6,000 tokens) and happened in all
  120 trials. Both conditions used the edited marker after the edit and after
  compaction, every time.
- **A smoke check** of plain and caller-conflict tasks, 3 pairs per model,
  stayed at the ceiling under both.
- **Cost per trial.** A cost 1-10% more than M on one- and two-turn trials. On
  the four-turn resume trials it cost about 27% more on Sonnet ($0.23 vs
  $0.18) and 39% more on Haiku ($0.013 vs $0.0095), because a resume renders
  the system prompt afresh. All costs are list
  prices as Claude Code reports them (`total_cost_usd`).

**The checks owed, 2026-11-23.** Claude Code 2.1.330, Sonnet
(`claude-sonnet-5-5`), the same throwaway clone, 17 runs, $0.87. The
invocation was the helper's as above, with `--permission-prompt-tool stdio`,
whose asks the harness answered with a denial, and the three flags. There was
no `--add-dir` for the role folder or the notes folder.

- **The notes folder by a `Read` rule.** The stand-in notes folder was outside
  the working folder and held a file with a random marker. The `--settings`
  file allowed it as `notes_allow` spells it (`Read(//e/.../agents/**)`).
  - In `default`, `acceptEdits` and `auto`, 2 runs each, the helper read the
    file and quoted the marker. It made no ask and had no
    `permission_denials`.
  - A file in a sibling folder outside the rule was asked for in `default` and
    `acceptEdits`. The denial came back as the `tool_result` and in
    `permission_denials`. In `auto` it was read unasked: that mode lets reads
    through anyway.
  - Without the rule, `default` asked for the notes file too, so the rule is
    what let the reads through.
- **Subagents see the role.** The parent was asked to give a general-purpose
  Task agent a fixed question: does it see a rule about a closing line?
  - With `--append-subagent-system-prompt-file` the subagent quoted the
    `ROLE-OK-<n>` rule with its digits in 3/3 runs.
  - With the append file alone it answered NONE in 3/3.
  - The digits were never in the parent's prompt to the subagent. The parent
    ended with its marker in all 6.
- **The hidden flag.** `--append-subagent-system-prompt-file` is absent from
  `--help`. A path that does not exist stops the CLI before any turn: exit 1,
  `Error: Append subagent system prompt file not found: <path>`. An option the
  CLI does not know stops it the same way (`error: unknown option`, exit 1).
  - Run as the probe runs the CLI, the bare `claude
    --append-subagent-system-prompt-file <missing path>` needed no sign-in.
    Its home was an empty scratch folder (`CLAUDE_CONFIG_DIR`, the working
    folder too), stdin was closed and `DISABLE_AUTOUPDATER=1` was set.
  - It ended in about 0.2 s with that error on stderr and nothing on stdout.
    It started no session: no `projects/` or `sessions/` folder and no
    transcript.
  - It left a first-start `.claude.json` and its backup in the scratch home.
    `--help` there leaves nothing.

Limits of the evidence:

- One CLI version, two models, synthetic rules.
- `role-helper.md`, not `role-lead.md`, and no constraints heading.
- One permission mode, on Windows.
- Scoring by script: language by word counts, files by the paths in tool
  calls. Spot reads agreed with it.
- The subagent result covers the general-purpose Task agent only.

## Consequences

- **The role holds against what the account and the project say.** Most
  clearly against memory that claims to override everything, where M lost
  almost always. A weaker model still follows the latest instruction when the
  caller and the account agree against the role (Haiku: 2/30 under A, 0/30
  under M). The role text cannot fix that; a caller's brief can.
- **The role folder is no longer a working folder.** Under M a helper could
  write into its own `hooks/<id>` folder, and twice it did. The threat model's
  rows on what a helper is given at launch are updated with the change.
- **A helper may mention an edited role.** After an edit mid-conversation, a
  helper under A sometimes notes that its earlier reply ended with "the wrong
  line" (Sonnet 17/30, Haiku 4/30, M 0/30). The new text is still the one it
  follows. This is cosmetic, but it is visible to its caller.
- **No `@` imports.** An `@` import in a library entry is no longer expanded
  for leads and helpers, as for chats since 2026-11-22: the system prompt takes
  the file as text.
- **Resumed helpers cost more**, by about a quarter on a resumed Sonnet
  helper, as measured above.
- **One of the flags is hidden.** `--append-subagent-system-prompt-file` is
  not in `--help` (2.1.330), so the offline probe (`cli/probe.rs`), which
  reads `--help`, cannot vouch for it that way. A version without the flag
  would refuse to start every helper: 2.1.330 exits on an option it does not
  know. A missing file stops it too, which the refusal to start a turn without
  its layers already covers.
  - Proposed resolution, for the operator to decide with this ADR: the probe
    checks the flag by how the CLI behaves, not by `--help`.
  - It runs `claude --append-subagent-system-prompt-file <a path that does not
    exist>` in its scratch home, as it runs its other offline commands. It
    requires the `Append subagent system prompt file not found` error.
  - `unknown option`, or any other answer, fails the probe. A version without
    the flag is then refused at install, like a version missing any other flag.
  - The live test still owes the rest: a helper's Task subagent reports a
    marker from its role. The `cli-upgrade` skill runs it on every new Claude
    Code version.
- **Code that goes** (from the static inventory):
  - In `background/command.rs`: the role `--add-dir`, the env assignment, and
    the `claude_memory_off` and `own` narrowing setup.
  - In `claude/launch.rs`: `instructions_applied`, `CLAUDE_ADDED_DIR_MEMORY`,
    `AddedDir` and `memory_excludes`.
  - The `memory_off` and `memory_kept` fields and their reporting, in
    `library/folders.rs` and `applied.rs`.
  - In `pty/start.rs`: `claude_settings_dirs`, `claude_memory_off`,
    `claude_added_dirs`, `added_dir` and its export in `pty/mod.rs`, and the
    added-dir branches of `claude_account_excludes` and `own_memory`.
- **Tests:**
  - The narrowing tests in `pty/start.rs` retire. Their literal-path,
    account-file and account-collision assertions are kept.
  - The launch tests and `<instructions-dir>` in `golden.rs` change.
  - The write-failure test in `background/command.rs` stays.
- **Docs that change with the code:**
  - the comments in `library/role.rs` and `library/mod.rs`;
  - `architecture/agents.md`, `library.md`, `providers.md` and
    `pty-and-launch.md`;
  - `user-guide/context.md`, `security/threat-model.md` and
    `cli-compatibility.md`;
  - a status line on ADR-0026 and ADR-0016.
- **Settled by the checks of 2026-11-23:**
  - A `Read` allow rule opens a lead's notes folder under `-p` in `default`,
    `acceptEdits` and `auto`, and the `--add-dir` fallback is not needed.
  - The subagent flag carries the role into a general-purpose Task agent;
    without it the agent does not see the role.
- **Owed before acceptance:**
  - The `luna-security-review` checklist for the changed launch argv.
  - The operator's decision on the probe's behavioral check for the subagent
    flag, proposed under "One of the flags is hidden".
- **Codex needs nothing.** `developer_instructions` is already Codex's
  counterpart of A: a developer message, outside `AGENTS.md`. The eval did not
  measure Codex.

## Alternatives weighed

- **Keep memory (M).** Refuted by the decisive run for both models wherever
  other memory contradicts the role.
- **Both channels, memory and append.** The text twice in every request, and
  the memory copy still loses to a stronger memory file. Not measured; nothing
  suggests it beats A.
- **Leave the account's `CLAUDE.md` out of helpers** (`claudeMdExcludes`).
  This removes the account conflict only. The account file holds the person's
  machine-wide rules, which ADR-0026 keeps as layer 1 for every role, and a
  project's own `CLAUDE.md` cannot be dropped this way.
- **Override wording in the role's memory file.** Every memory block would
  then claim to win, an arms race with whatever the person or a project
  writes. Not measured.
- **`--system-prompt` or its file.** It replaces Claude Code's own base
  prompt, tools guidance included.
- **The role in the turn's prompt** (before 0.18). ADR-0026 moved away from
  it. It is text in the conversation, so a compaction may summarize it away.

## Reversal

The change lives in the launch arguments and their reporting. No stored data
or account file changes shape, so reverting the implementation brings back the
memory path at a helper's next turn:

- The layer file is written every turn under both designs.
- With snapshot `off`, no conversation is left holding a recorded prompt.
- A helper resumed after the revert reads its layers as memory again, and its
  history keeps the earlier replies.

Reverse if a Claude Code version drops or changes the append flags or the
snapshot switch. The probe and the live suite report that. Reverse also if a
measurement like the one above shows memory holding the role better on a
model Luna's helpers use.
