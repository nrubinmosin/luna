---
name: live-test
description: Run Luna's live suite (scripts/live-tests.ps1, src-tauri/src/live.rs) against the real Claude Code and Codex CLIs on throwaway accounts cloned into test-build/accounts, and prove the source account came through untouched. Use it whenever someone asks to run the live tests or a live check, to test against the real CLIs or with a signed-in account, to confirm a CLI update or a launch/hub/MCP change works for real, before a release, or asks to clone a test account, even if they only say "check it live" or "smoke it on real Claude/Codex".
---

# Live test

The live suite spends real quota and runs real CLIs that write into their
account folder, so the work around it matters as much as the run: a throwaway
account, proof that the real account was not touched, cleanup, and a report
that separates Luna's failures from a model's. The levels and the reasons are
in `docs/testing.md` ("3. Live"); read it once if it is new to you.

## 1. Accounts

Find what is there first: `./scripts/test-accounts.ps1 list` (names, providers,
signed in or not). Reuse a signed-in clone when one exists; it saves a login.

Otherwise clone, one per provider the run needs:

```powershell
./scripts/test-accounts.ps1 clone -Source <account folder> -Provider anthropic -Name live
./scripts/test-accounts.ps1 clone -Source <account folder> -Provider openai -Name live
```

- **Default: without credentials.** The clone prints the one step that signs
  the copy in (`CLAUDE_CONFIG_DIR=... claude` then `/login`, or
  `CODEX_HOME=... codex login`). That login is a person's step: hand it to
  them and wait. Do not try to complete it yourself.
- **`-WithCredentials` only when the person explicitly asks for it**, and say
  why it is risky before running it: OAuth refresh tokens rotate, so whichever
  of the two folders refreshes first leaves the other with a dead token; a live
  run on the copy can sign the original out. If they still want it, go ahead
  and note it in the report.
- The source must be a real account folder the person named. Never pick one
  yourself, and never point anything at Luna's own install (`E:\Programs\Luna`)
  or its data folder.

## 2. Fingerprint the source before

Hash the source account's credential file (`.credentials.json` for Claude
Code, `auth.json` for Codex) and record the hash and timestamp, not the
contents. Never print, copy into a report, or log a credential file's
contents.

```powershell
$cred = Join-Path <source> '.credentials.json'   # or auth.json
$before = if (Test-Path $cred) { (Get-FileHash $cred -Algorithm SHA256).Hash } else { 'absent' }
$beforeTime = if (Test-Path $cred) { (Get-Item $cred).LastWriteTimeUtc.ToString('o') } else { '' }
```

## 3. Run

```powershell
./scripts/live-tests.ps1 -ClaudeAccount live -CodexAccount live    # both
./scripts/live-tests.ps1 -ClaudeAccount live -NoBuild              # one provider, last build
```

Name only the providers the change touches; each extra one costs quota. The
script refuses an account that is missing, linked or signed out, builds the
Windows test executables (Docker) into `test-build/live-tests`, and runs only
the live tests, one at a time. It takes minutes; run it in the background and
wait for it rather than polling.

## 4. Compare after

Hash the same file again. Same hash: the source was not touched. Different:
say so first in the report, because it means the source's login may have been
rotated and the person should check that account still works. With
`-WithCredentials` a rotation is the known risk; without it, any change points
at something writing outside `test-build/accounts` and is a finding in itself.

## 5. Read failures before blaming code

The answers are a model's. On a failure, read `test-build/live-tests/luna.log`
and the test's panic message: a refused flag, a missing stream event or an
auth error is Luna's (or the CLI's) problem; an off-topic answer is probably
the model. Rerun a model-shaped failure once; do not change code on one red
run.

## 6. Clean up

A signed-in clone is worth keeping for the next run, since signing in again
costs the person a step; delete it with `./scripts/test-accounts.ps1 clean
-Name live` when they asked for it gone or it was cloned with credentials (a
second live copy of a login keeps the rotation risk alive). Say which you kept.
The tests delete their own helpers and work folders; stop anything else of
yours you started.

## Report

- Per provider: passed / failed, the test names, CLI version used (the script
  prints which CLI it took).
- Evidence: the script's summary line, and for a failure the relevant log
  lines and your reading of them (Luna, CLI or model).
- Credential hash: unchanged / changed, per source account, and whether the
  clone had credentials copied.
- What was left behind (kept clones) and what was cleaned.
