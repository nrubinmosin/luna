---
name: security-review
description: Security checklist for a Luna change, built from docs/security/threat-model.md — which trust boundary (B1–B7) it crosses, which STRIDE rows and MCP authorization rows it touches, which negative test it needs, when the threat model itself must change — plus an independent adversarial review. Use it whenever a change touches the hub or its listeners (hub.rs, admit.rs, coreproc.rs, ipc.rs), MCP tools or their authorization (mcp.rs, agents/registry.rs, agents/privilege.rs, agents/answer.rs, agents/spawn/), the least-privilege layer, Tauri IPC commands, capabilities or the CSP, how a CLI process is launched, private files and ACLs (secret.rs, paths.rs), worktree or account-folder writes, the updater or the installer (hooks.nsh) — before calling such a change done, or when someone asks "is this safe?" or for a security review of Luna code.
---

# Security review

`docs/security/threat-model.md` is the map: the assets, the actors, the trust
contract for Luna's tools, a STRIDE table per boundary, the MCP authorization
matrix, the limits of the least-privilege layer, the findings and the accepted
risks. A review places the change on that map, checks it against the rows it
lands on, proves the refusals with tests, and keeps the map true.

## 1. Place the change

From the diff (`git diff master...`), list every boundary it crosses:

| Boundary | Code that puts a change on it |
|---|---|
| B1 WebView → window (Tauri IPC) | `#[tauri::command]` fns, `capabilities/*.json`, `tauri.conf.json` CSP, anything rendering text as HTML in `src/` |
| B2 window → core (loopback TCP) | `ipc.rs`, `coreproc.rs`, `corecmd.rs` dispatch, `corelink.rs` |
| B3 sessions → hub (hooks, guard, MCP) | `hub.rs`, `admit.rs`, `mcp.rs`, `agents/registry.rs`, `agents/privilege.rs`, `agents/answer.rs`, `agents/spawn/` |
| B4 core → CLI processes | `pty/spawn.rs`, `pty/start.rs`, `background/command.rs`, `*/launch.rs`, text Luna types into a session |
| B5 core → account and project folders | `library/`, `worktree/`, `accounts.rs`, `*/trust.rs`, `*/config.rs`, `claude/oauth.rs` |
| B6 updater | `tauri.conf.json` updater, release signing |
| B7 managed CLI downloads | `cli.rs`, `claude/cli.rs`, `codex/cli.rs` |

Also: private files and ACLs (`secret.rs`, `paths.rs`, F4) and the installer
(`src-tauri/windows/hooks.nsh`, §7.1). If none applies, say so and stop.

## 2. Check each row it lands on

For each boundary, read its STRIDE table and ask of the diff:

- Does a "mitigated" or "fixed" row still hold? The row names its mechanism and
  file; confirm the change keeps it on every path, including new ones (a new
  MCP tool, a new command, a new spawn path, a new file written).
- New MCP tool or a new target for an old one: which row of the §5 matrix
  covers it, which check (`authorize`, `authorize_send`, `authorize_view`,
  `answer_reach`, `cap`, `reaches_luna`) guards it, and does it respect the
  trust contract in §3 (a caller never acts above its own privilege; `/` and
  `!` text only from full access into its own helper)?
- New input from a less trusted side: is it bounded (size, time, count) and
  parsed strictly, as B2/B3's H2 rows require?
- New file with a secret or state another user could read or replace: created
  private (`secret::create_private` / `open_private`), no link followed?
- Launch: argv only, never a shell; values checked against fixed lists;
  inherited session variables stripped; no new token on a command line.
- Does it widen a §6 limit or touch an §8 accepted risk? Then it is a decision
  for the person, not a detail.

## 3. Negative tests

Each refusal the change relies on gets a test that the forbidden thing is
refused, beside the code, as the existing ones do (`agents/privilege.rs`,
`agents/answer.rs`, `hub.rs`, `mcp.rs`, `secret.rs` tests): e.g. a caller one
rung below asking for more, a token of another session, an oversized or slow
body, a link where a folder should be. A matrix row whose "Tested" would turn
"no" is a gap to close or to name.

## 4. Independent adversarial review

Your own reading is anchored on your intent. Get a second one from a reviewer
that did not write the change, ideally the other model family (the
`luna-orchestrator` skill says how where it is available; otherwise a fresh
subagent). Give it the diff, the boundaries from step 1 and the threat model
path, not your conclusions, and ask it to break the change: find a path from
a less trusted actor (§3) to an asset (§2) the change opens or leaves open.
Check each claim it makes in the code before accepting or rejecting it.

## 5. Keep the threat model true

Update `docs/security/threat-model.md` in the same change when it adds a tool,
command, listener, file or download; moves a mitigation (line references
included); fixes or opens a finding; or changes what §5 or §6 says. A new
residual goes in §7 or §8 with why it is accepted.

## Report

Boundaries crossed; per STRIDE / matrix row: holds, changed (how), or broken
(with the path); negative tests added; what the independent reviewer found and
what you made of each point; threat-model edits; anything left for the
person's decision.
