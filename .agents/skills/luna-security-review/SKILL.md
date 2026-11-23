---
name: luna-security-review
description: Security checklist for a Luna change, built from docs/security/threat-model.md — which of Luna's own trust boundaries it crosses, which STRIDE rows and MCP authorization rows it touches, which negative test it needs, when the threat model itself must change — plus an independent adversarial review. Luna answers for its own surfaces only (the hub's token and admission, a session's reach over its subtree, its files and their ACLs, the window–core link, the updater and CLI downloads), not for the CLIs' modes, sandboxes or network. Use it whenever a change touches the hub (hub.rs, admit.rs), the core's endpoint or its peer check (endpoint/, peer/, coreproc.rs, corelink.rs, corecmd.rs, ipc.rs), MCP tools or who may call them (mcp/, agents/admission.rs, agents/registry.rs, agents/answer.rs, agents/handover.rs, agents/spawn/), the native window's commands (native.rs and native/, the window's own and those it passes to the core), how a CLI process is launched (argv, environment, files handed to it), private files and ACLs (secret.rs, paths.rs, boundary.rs), account-folder or library writes, the updater or the installer (installer.nsi, hooks.nsh) — before calling such a change done, or when someone asks "is this safe?" or for a security review of Luna code.
---

# Security review

`docs/security/threat-model.md` is the map: the assets, the actors, the trust
contract for Luna's tools, a STRIDE table per boundary, the MCP authorization
matrix, the limits of Luna's admission, the findings and the accepted risks.
A review places the change on that map, checks it against the rows it lands
on, proves the refusals with tests, and keeps the map true.

## What Luna answers for, and what it does not

Since the 2026-11-20 amendment of `docs/adr/0030-scope-of-0-20.md`, Luna is a
runtime for Claude Code and Codex that imposes as little of its own as it
can. It answers for **its own surfaces**:

- the hub's tokens and who it admits to which tool, and a session's reach
  over the sessions under it (its subtree);
- Luna's files and their ACLs: the data folder, the core's endpoint, the
  secrets Luna keeps;
- the link between the window and the core;
- the updater and the CLI downloads.

It guarantees **nothing about the CLIs' own security**: their permission
modes, sandboxes and network are theirs, run as their settings and the flags a
session asked for say. There is no ladder ranking modes (`agents/privilege.rs`
is gone; admission is `agents/admission.rs`), and what a CLI observably does
is recorded in `docs/cli-compatibility.md`, not claimed as Luna's.

This decides what a finding is. A change that passes a mode, an approval or a
sandbox flag a caller asked for is reviewed for Luna's side only: is the value
spelled exactly or taken from a fixed list, does it go as an argv element
never through a shell, is it the right session that may name it (admission),
and does Luna's text claim nothing the CLI does not guarantee. "The CLI's
sandbox does not contain X" or "the helper could do more than its parent" is
not a finding against Luna: say it is the CLI's and leave it out of the
verdict. Do not ask for a cap, a comparison or a guarantee the amendment
removed. What does stay Luna's: no helper rooted in, inside or above the
data folder (`reaches_luna`), text a CLI reads as a command going only into
one's own helpers (`command_reach`), no token or hub secret on a command line
or in a log.

## 1. Place the change

From the diff (`git diff master...`), list every boundary it crosses. Read the
headings of §4 of the threat model for the current numbering (trust its
numbers over this table's):

| Boundary | Code that puts a change on it |
|---|---|
| B1 (retired) the WebView page; the native window has no page: text from a CLI is drawn, and its terminal only answers a program's queries back into that session (`crates/window/src/term/mod.rs`, `UiMsg::Reply`; `docs/adr/0030-scope-of-0-20.md`) | nothing: a change that brings back a page, a web view or markup rendered from a CLI's text is a new boundary, for the person to decide |
| B2 window → core (the core's endpoint: a named pipe on Windows, a Unix socket elsewhere; no token, the OS-attested peer is the key), and the window's own commands, run in the window process | `endpoint/`, `peer/`, `ipc.rs`, `coreproc.rs`, `corecmd.rs` (`COMMANDS`, dispatch), `corelink.rs` (`passes`) and `corelink/v1.rs`, `native.rs` (`dispatch`, `OWN`, `OWN_SENT`) and `native/` (`panels.rs`, `dialogs.rs`, `desktop.rs`), `boundary.rs` (account and folder path arguments) |
| B3 sessions → hub (hooks, MCP): who is admitted to which tool, and how far | `hub.rs`, `admit.rs`, `mcp/` (`dispatch.rs`, `args.rs`), `agents/registry.rs`, `agents/admission.rs`, `agents/answer.rs`, `agents/handover.rs`, `agents/spawn/` |
| B4 core → CLI processes: how they are started, not what their mode allows | `pty/spawn.rs`, `pty/start.rs`, `background/command.rs`, `claude/launch.rs`, `codex/launch.rs`, text Luna types into a session |
| B5 core → account and project folders | `library/`, `accounts.rs`, `account_settings/`, `claude/trust.rs`, `codex/trust.rs`, `codex/config.rs`, `claude/oauth.rs` |
| B6 updater | `desktop/updater.rs` (its key in `src-tauri/updater.json`), `windows/installer.nsi`, release signing |
| B7 managed CLI downloads | `cli.rs`, `claude/cli.rs`, `codex/cli.rs` |

Also: private files and ACLs (`secret.rs`, `paths.rs`, F4), the installer
(`src-tauri/windows/installer.nsi`, `hooks.nsh`, §7.1), the single-instance
hand-over (`desktop/single.rs`, R9), the import of 0.19's WebView state
(`native/import.rs`, `native.rs` `window_state`) and the terminal's answers to
a program's queries (`crates/window/src/term/`). If none applies, say so and
stop. Grep every path above against the tree before relying on it:
code is still moving, and a path that is gone means the row naming it is
stale too.

## 2. Check each row it lands on

For each boundary, read its STRIDE table and ask of the diff:

- Does a "mitigated" or "fixed" row still hold? The row names its mechanism and
  file; confirm the change keeps it on every path, including new ones (a new
  MCP tool, a new command, a new spawn path, a new file written).
- New MCP tool or a new target for an old one: which row of the §5 matrix
  covers it, which check guards it (`authorize`, `authorize_send`,
  `authorize_view`, `write_reach`, `in_scope`, `scope_all_in`, `beneath`,
  `persons_chat_only`, `answer_reach`, `command_reach`, `reaches_luna`), and
  does it respect the trust contract in §3: a caller acts only on the sessions
  under it, writes go in only from above (ADR-0025, ADR-0028), a helper never
  reaches `scope: "all"`, a person's chat is never anyone's descendant? A
  change that widens who may act on whom (a lead answering a grandchild, a
  sibling writing, a helper notifying the person) changes the admission
  model: that is a decision for the person, with the matrix row and its test
  updated, not a detail.
- A new window command or core command: the list it joins (`corecmd::COMMANDS`
  and its lane, which `corelink.rs` `passes` and the core's endpoint
  (`coreproc.rs` `off_lane`) both hold each frame to; or `OWN` / `OWN_SENT` in `native.rs` or `native/` for one the
  window process runs itself, with `every_command_the_window_names_is_dispatched`),
  whether its path arguments are `AccountPath` / `AbsPath`, what it can write
  into the data folder or an account and whether it can be steered through a
  link. Any same-user process the endpoint admits can send a core command,
  not only the window, each on its own lane (the core holds every frame to
  its command's lane, `coreproc.rs` `off_lane`), say it is a window and hear every event, sessions' output
  among them, and ask the protocol's `shutdown` (threat model B2).
- New input from a less trusted side: is it bounded (size, time, count) and
  parsed strictly, as B2/B3's H2 rows require?
- New file with a secret or state another user could read or replace: created
  private (`secret::create_private` / `open_private` / `write_private`), no
  link followed, ACL checked where the threat model says it is?
- Launch: argv only, never a shell; values checked against fixed lists;
  inherited session variables stripped; no token or hub secret on a command
  line or in `luna.log`.
- Does it widen a §6 limit or touch an §8 accepted risk? Then it is a decision
  for the person, not a detail.

## 3. Negative tests

Each refusal the change relies on gets a test that the forbidden thing is
refused, beside the code, as the existing ones do (`agents/admission.rs`,
`agents/answer.rs`, `agents/registry.rs`, `hub.rs`, `mcp/`, `secret.rs`
tests): e.g. a caller outside the target's subtree, a write from a sibling or
from below, a token of another session, an oversized or slow body, a command
the allowlist does not name, a link where a folder should be. A matrix row
whose "Tested" would turn "no" is a gap to close or to name.

## 4. Independent adversarial review

Your own reading is anchored on your intent. Get a second one from a reviewer
that did not write the change, ideally the other model family (a fresh
subagent or the other CLI, where available). Give it the diff, the boundaries
from step 1 and the threat model path, not your conclusions, and ask it to
break the change: find a path from a less trusted actor (§3) to an asset (§2)
the change opens or leaves open, through Luna's own surfaces. Check each claim
it makes in the code before accepting or rejecting it; a claim that rests on a
CLI's mode or sandbox being weak is out of scope (see above).

## 5. Keep the threat model true

Update `docs/security/threat-model.md` in the same change when it adds a tool,
command, listener, file or download; moves a mitigation (line references
included); fixes or opens a finding; or changes what §5 or §6 says. A new
residual goes in §7 or §8 with why it is accepted.

## Report

Boundaries crossed; per STRIDE / matrix row: holds, changed (how), or broken
(with the path); what is the CLI's and so out of scope, named as such;
negative tests added; what the independent reviewer found and what you made
of each point; threat-model edits; anything left for the person's decision.
