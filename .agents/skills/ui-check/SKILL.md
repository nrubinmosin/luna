---
name: ui-check
description: Check a Luna UI change the way the repo allows — the frontend gate, headless screenshots of the demo scenes before and after (scripts/screenshots.ps1) compared side by side, and a smoke run of the test build in the background, driven only through its own windows (WebDriver on its WebView, the dev caller), never by global input. Use it whenever a change touches src/ (components, stores, CSS, themes, layout, the demo data) or a window/tray behavior, when someone asks to see, screenshot, compare or smoke-test the UI, to check a change "looks right" or fits a small screen, or to retake the README screenshots, even if they only say "does the settings page still look ok?".
---

# UI check

Three levels, cheapest first; stop at the one that answers the question, and
say which you ran.

## 1. Frontend gate

```powershell
./scripts/gate.ps1 -Group frontend
```

Formatting, lints, types, `pnpm check:text`, the Vitest tests and the build,
in Docker. A new component or store behavior gets its Vitest test beside the
existing ones; a whole screen can use the demo data in `src/dev/demo/`.

## 2. Screenshots, before and after

`scripts/screenshots.ps1` renders the frontend alone (Tauri commands are
no-ops) on the invented data in `src/dev/demo/` with headless Chromium in
Docker. It never touches a running Luna, so it is the safe way to see the UI.

- Pick the scenes the change shows up in: the names are the `Scene` type in
  `src/dev/demo/scenes.ts` (`main`, `dark`, `newchat`, `settings`,
  `accounts`, `checks`, ...). A scene whose name ends in `dark` renders the
  dark theme; only some have a dark twin. If no scene shows the change (say,
  a window in dark), adding one to `scenes.ts` is part of the change: propose
  it in the plan.
- **After** (your tree):
  `./scripts/screenshots.ps1 -Out test-build/shots/after -Scenes settings,dark`
- **Before** (the base commit): the same scenes from a detached worktree
  under `test-build/`. `-Out` is relative to the checkout the script runs
  from, so move the shots out before removing the worktree:

  ```powershell
  git worktree add --detach test-build/ui-before master
  ./test-build/ui-before/scripts/screenshots.ps1 -Out shots -Scenes settings,dark
  Move-Item test-build/ui-before/shots test-build/shots/before
  git worktree remove test-build/ui-before
  ```

  A scene the change adds has no before; say so and judge it from the diff.
- Always pass `-Out` under `test-build/`: the default is `docs/screenshots`,
  the README's images, which change only when retaking them is the task.
- Compare: hash each pair to find which changed (`Get-FileHash`), then look at
  the changed pairs yourself (open both images). Report what differs and
  whether each difference is the one intended; an unchanged scene that should
  have changed is a finding too.
- **A layout change** (a dialog, popover, menu or panel that grows, moves or
  scrolls differently) also runs `./scripts/fit-check.ps1`: every scene and the
  popups a click opens, at 1280×720 and 1024×600, 125% and 150%. It fails on a
  control that is neither on screen nor in a scroller that is, and on a popup
  cut off where it stands; shots and `report.json` go to `test-build/fit`, and
  `-Scenes`/`-Sizes` narrow a rerun. A screen it cannot reach gets a scene in
  `scenes.ts` or an entry in the script's `EXTRAS`. To shoot one size,
  `screenshots.ps1 -Size 853,480 -Scale 1.5` is a 1280×720 screen at 150%.

## 3. Test-build smoke

For what the demo cannot show: real windows, the tray, Tauri commands, sessions.

```powershell
./scripts/test-build.ps1 -Run -Background
./scripts/test-build.ps1 -Run -DevCaller -DevFolder <folder> -DevAccount test-build/accounts/<provider>/<name>
```

The test build keeps its own log, settings and CLIs in `test-build/` and runs
beside the Luna in use. `-Background` keeps its window off screen, unfocused
and off the taskbar; `-DevCaller` implies it.

Someone is using this desktop. Reach the test build only through its own
windows:

- **WebDriver / DevTools on its WebView** (preferred for UI): start it with
  `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=<port>` in the
  environment of that launch, then drive and screenshot the page over that
  port (msedgedriver or the DevTools protocol). Untested with Luna so far:
  the switch is WebView2's own, but nothing in the repo uses it yet, so say
  whether it worked.
- **The dev caller** for sessions and Luna's tools: the token
  `-DevCaller` prints and the MCP URL in `test-build/luna.log`; call the tools
  over HTTP.
- UI Automation patterns on its own elements, or messages to its own window
  handles, if the above cannot reach something.

Never synthetic input (SendInput, keybd_event, mouse_event, SendKeys, cursor
moves), never SetForegroundWindow, nothing aimed at a window that is not the
test build's, and never the Luna in use (`E:\Programs\Luna`). If a check
cannot be done without that, stop and report what is left unchecked instead.

Accounts the test build writes into are throwaway ones under
`test-build/accounts` (the `live-test` skill makes them). When done, stop the
test build's own processes only, matched by path (`test-build\luna-test.exe`),
and anything else you started.

## Report

Which levels ran and their result; per scene: changed or not, intended or
not, with the shot paths under `test-build/shots/`; the fit check's verdict
for a layout change; the smoke steps done and what was seen; anything left
unchecked and why.
