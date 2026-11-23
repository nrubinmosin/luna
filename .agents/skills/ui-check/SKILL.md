---
name: ui-check
description: Check a Luna UI change the way the repo allows — the window crate's tests and fit test in the gate, offscreen shots of the native window's demo scenes before and after (scripts/gpui-shots.ps1) compared side by side, the fit check for anything that sizes or scrolls, and a smoke run of the test build in the background, driven only through its own test driver and windows, never by global input. Use it whenever a change touches the native window (src-tauri/crates/window: views, panels, dialogs, theme, colors, layout, the demo scenes) or its host (native.rs, native/), or a window/tray behavior, when someone asks to see, screenshot, compare or smoke-test the UI, to check a change "looks right" or fits a small screen, or to retake the README screenshots, even if they only say "does the settings page still look ok?".
---

# UI check

The window is native (GPUI, `src-tauri/crates/window`, hosted by
`src-tauri/src/native.rs`). Four levels, cheapest first; stop at the one
that answers the question, and say which you ran.

## 1. Tests and the gate

The window crate's tests (`tests.rs`, `board_tests.rs`, the tests inside
its modules) and the fit test (`fit_tests.rs`) run in the Rust gate:

```powershell
./scripts/gate.ps1 -Group rust
pwsh -File scripts/check-text.ps1      # US English, on the host
```

The fit test needs a debug build (selectors are recorded only there), so it
runs in the Linux and macOS jobs, not in the release-built Windows tests.
CI runs the same on every push of the branch; every local Docker build
holds the machine-wide lock, so locally run the narrowest step that answers
you and let CI be the gate (`./scripts/ci-wait.ps1`). A new behavior of a
view or a widget gets its test beside the existing ones.

## 2. Shots, before and after

`scripts/gpui-shots.ps1` opens each scene of the window's demo
(`crates/window/src/demo.rs`, with the panels', dialogs' and status bar's
own `demo::scenes`) over a fake host, off screen and never activated, and
renders it to `<scene>-<light|dark>.png`. It touches no running Luna and no
other window, so it is the safe way to see the UI.

- Pick the scenes the change shows up in: `-Scenes` keeps those whose name
  *contains* the string (`dialog-new-chat` also shoots
  `dialog-new-chat-codex`, `-models`, ...). It takes one string: for
  several, run again with `-NoBuild`. Every scene comes in both themes.
- **A new screen or popup gets a scene** in the demo, in the same change: a
  panel or dialog as driver lines in its module's `demo::scenes` (the same
  lines the test build's driver takes, below). A popup (a panel, dialog,
  menu, drop-down) also gets its `OPENS` entry in `fit_tests.rs`: the
  scene, a control it must show, and its frame's selector (`popup(...)`),
  or the fit test fails with "opens something but OPENS names nothing".
- **After** (your tree; `-Out` is relative to the checkout's root):

  ```powershell
  ./scripts/gpui-shots.ps1 -Out test-build/shots/after -Scenes settings
  ```

- **Before**: the same scenes from a detached worktree of the base commit
  under `test-build/`: `HEAD` while the change is uncommitted, else the
  commit your branch started from (`git merge-base HEAD <base branch>`;
  not `master` on a branch whose base is another one). It builds its own harness from cold
  the first time, so keep it while you iterate and remove it when done;
  move its shots out first:

  ```powershell
  git worktree add --detach test-build/ui-before <base commit>
  ./test-build/ui-before/scripts/gpui-shots.ps1 -Out shots -Scenes settings
  Move-Item test-build/ui-before/shots test-build/shots/before
  git worktree remove test-build/ui-before
  ```

  A scene the change adds has no before; say so and judge it from the diff.
- **Compare side by side**: hash each pair to find which changed
  (`Get-FileHash`), then open both images of every changed pair yourself
  and look. Report what differs and whether each difference is the one
  intended; a scene that should have changed and did not is a finding too.
- Keep `-Out` under `test-build/` (its default, `test-build/shots/gpui`):
  `docs/screenshots/` changes only when retaking the README's pictures is
  the task (below).

## 3. Fit on small screens

A change that sizes, moves or scrolls something (a dialog, panel, popover,
menu, a row's buttons, a long label) runs:

```powershell
./scripts/gpui-shots.ps1 -Fit
```

First the fit test in the Linux image: every light scene and the popup it
opens, at each of `SIZES` (down to 1024×600 at 150%, 683×400 logical), every
control with a selector must be on the window, not cut off by a frame round
it unless the wheel scrolls it whole into view, and reached by a hit test.
Then every scene is shot at those sizes into `<Out>/fit/` to look at;
`-Scenes` narrows only these shots, never the test.
What is out of reach today is listed exactly in `KNOWN`, and the test fails
on a listed control that is in reach too: **a fix takes its entry off
`KNOWN`** (or the selectors, scenes or size it no longer covers), in the
same change. Never add to `KNOWN` to make a new problem pass; fix it, or
stop and ask.

## 4. Test-build smoke

For what the demo cannot show: a real core, sessions, the tray, the
updater, the window's real state.

```powershell
$env:LUNA_TEST_DRIVE = "$PWD/test-build/drive.txt"
./scripts/test-build.ps1 -Run -Background
./scripts/test-build.ps1 -Run -DevCaller -DevFolder <folder> -DevAccount test-build/accounts/<provider>/<name>
```

The test build keeps its own log, settings and CLIs in `test-build/` and runs
beside the Luna in use. `-Background` keeps its window off screen, unfocused
and off the taskbar; `-DevCaller` implies it. The driver, the test clipboard
and the tray hook exist in the test build alone (the `luna_test` cfg).

Someone is using this desktop. Reach the test build only through what is
its own:

- **The driver** (`LUNA_TEST_DRIVE`, set in the environment the test build
  is started from; `crates/window/src/drive.rs` lists every line, the
  panels' and dialogs' own in their `drive` functions): append one line
  at a time, newline included, and the window acts on it within 200 ms as
  if it had been clicked or typed into. `keys ctrl-c enter`, `text words`,
  `focus 2`, `row 3`, `put 2 3`, `dialog new-chat`, `newchat folder <path>`,
  `panel library`, `setting sound off`, `close`, `quit`. To see what
  happened, `dump`, `status`, `panels` and `dialogs` write the board's,
  status bar's, panel's and dialog's state into `test-build/luna.log`
  (lines starting `drive:`); read them there.
- **The test clipboard**: `LUNA_TEST_CLIPBOARD` names a JSON file standing
  in for the clipboard, so a paste never reads the real one
  (docs/development.md, "Native window shots").
- **The tray's test hook**: `WM_APP + 0x4C` to the test build's own tray
  window, lParam the number in `test-build/test-hook.secret`
  (architecture/native-window.md, "Desktop services").
- **The dev caller** for sessions and Luna's tools: the token `-DevCaller`
  prints and the MCP URL in `test-build/luna.log`; call the tools over
  HTTP (`luna_message_list` with `screen` reads a session's terminal).

Never synthetic input (SendInput, keybd_event, mouse_event, SendKeys, cursor
moves), never SetForegroundWindow, never a window brought on screen,
nothing aimed at a window that is not the test build's, and never the Luna
in use (`E:\Programs\Luna`). How the window looks is the shots' job, not a
screen capture of the test build. If a check cannot be done without
crossing these lines, stop and report what is left unchecked.

Accounts the test build writes into are throwaway ones under
`test-build/accounts` (the `live-test` skill makes them). When done, stop
the test build's own processes only, matched by path
(`test-build\rm-luna-test.exe`), and anything else you started; unset the
variables you set.

## Retaking the README's pictures

Only when that is the task. Shoot every scene into a folder of its own, then
copy three of them over the pictures (docs/development.md, "Screenshots"):

```powershell
./scripts/gpui-shots.ps1 -Out test-build/shots/readme
Copy-Item test-build/shots/readme/chats-4-light.png docs/screenshots/panes.png
Copy-Item test-build/shots/readme/chats-2-dark.png docs/screenshots/dark.png
Copy-Item test-build/shots/readme/dialog-new-chat-light.png docs/screenshots/new-chat.png
```

Check that the three source files exist first (a renamed scene breaks this
silently), then open the new pictures and the old ones side by side before
committing.

## Report

Which levels ran and their result; per scene: changed or not, intended or
not, with the shot paths under `test-build/shots/`; the fit check's verdict,
and any `KNOWN` or `OPENS` entry added or removed; the smoke's driver lines
and what the log showed; anything left unchecked and why.
