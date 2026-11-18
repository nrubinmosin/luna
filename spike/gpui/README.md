# GPUI spike

A throwaway spike: GPUI from crates.io (`gpui-pre`, the published snapshot of
Zed's gpui) plus `gpui-component`, as a release Windows build cross-compiled
from Linux, run off screen and photographed without touching the desktop.
Lives in `test-build/` (git-ignored by Luna); nothing in Luna depends on it.
Its own git repo, local only.

## Run

```
spike.exe                    a normal window, centered, focused
  --background               off every display, never activated, not on the taskbar
  --exit-after SECS          quits by itself
  --size WxH                 window size in logical px (default 800x500)
  --hidden                   opened with show: false
  --shot PATH                (spike-shot.exe only) renders its frame to a PNG in-app and quits
                             (with --editor, also the editor's, to <stem>-editor.png)
  --editor PATH              also opens PATH (Markdown) in an editor window
  --stats PATH               writes a JSON line of stats to PATH when --exit-after quits
  --log LEVEL                a logger on stderr (error..trace), for gpui's own warnings
  --diag PATH                JSON lines (src/diag.rs): GPU, display and fonts at start, then
                             one a second: app/window state, display-link ticks, renders,
                             notifies, feeder ticks, foreground timer lateness
  --no-app-nap, --activate   macOS only (ignored elsewhere): an NSProcessInfo activity that
                             keeps App Nap off; the app activated at start
```

`pwsh ./shot.ps1` starts it with `--background`, finds its window by its own
PID, photographs it with `PrintWindow(PW_RENDERFULLCONTENT)` on that HWND into
`results/spike.png`, checks the picture is not blank, logs the foreground
window before and after (`results/spike-shot.log`) and kills its own process.
It sends no input and activates nothing.

`spike-shot.exe` (`pwsh ./build.ps1 -Features shot`) adds `--shot PATH`:
`Window::render_to_image`, which gpui-pre keeps behind `test-support` (the
feature turns it on for gpui and gpui_platform, so this exe carries test
code; `spike.exe` does not). With `--background` it gives the client area,
800x500, the same picture as PrintWindow's (`results/inapp-background.png`).
With `--hidden` (`show: false`) it gives a 1x1 image
(`results/inapp-hidden.png`). Why, from a short look: the renderer starts at
1x1 and is resized only on WM_SIZE, and upstream keeps a hidden window's
placement until `activate()`, so a hidden window gets no WM_SIZE. From the
spike's own code (no patch), `SetWindowPos(SWP_NOACTIVATE | SWP_NOMOVE |
SWP_NOZORDER)` on its own HWND does give the renderer the right size, but the
picture stays blank (the theme's background only), even with `Window::draw`
called right before `render_to_image`: nothing is painted into a window that
was never shown. Making it work would take a patch, so the experiment was
dropped; capture a shown, off-screen window instead.

## Results

Final figures, 2026-11-18/19, this PC (one 164 Hz display setup, Windows 11).
`spike.exe` is master (frames on demand), `spike-base.exe` the branch
`base-vsync` (the same tree without patch 3).

Patches (`git grep "gpui-spike patch"`, details under "Patches"):

| Patch | Files | Markers |
| --- | --- | --- |
| 1. Shaders at runtime | `directx_renderer.rs` | 1 |
| 2. Unattended windows | `window.rs` | 3 |
| 3. Frames on demand | `window.rs` 2, `platform.rs` 3, `events.rs` 2, `direct_manipulation.rs` 1 | 8 |
| Total | 5 files | 12 |

**Terminal burst on a quiet desktop** (`results/ab/quiet.log`,
`results/ab/quiet.json`, per run `results/ab/quiet/`): `ab-burst.ps1`, base
`bd94038` and fd `7cc2d58`, interleaved, 4 rounds of `burst:60` and one of
`burst:60x4`. Each run waited until every process but the soaks had stayed
under 15% of all cores for 2 min: 3.0-8.8% in those 2 min, 2.6-7.4% during
the runs (a browser the main consumer). Medians, ms:

| Feed | Variant | frame max | frame p99 | latency max | latency p99 | late (16.7 ms) | late (one 164 Hz vsync) | unshown at end | checksum |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| burst:60 x4 runs | base | 14.92 | 10.23 | 8.46 | 6.15 | 0 | 67 | none | 4/4 |
| burst:60 x4 runs | fd | 14.33 | 10.14 | 7.95 | 6.14 | 0 | 63.5 | none | 4/4 |
| burst:60x4, 1 run | base | 13.37 | 11.16 | 8.22 | 6.16 | 0 | 69 | none | 1/1 |
| burst:60x4, 1 run | fd | 11.23 | 10.59 | 6.40 | 6.11 | 0 | 62 | none | 1/1 |

On a quiet desktop the two do not differ beyond run-to-run noise; base had
the one outlier (round 3: 2 late frames, a 124 ms latency). Late against one
164 Hz vsync (6.1 ms) is 50-86 of ~3000 frames in every run of both: latency
p99 sits right at one vsync. Under load (a game, desktop 80-97%, "Burst under
load, A/B" below) all variants had a few late frames.

**4-hour soaks**, `--term --pty --editor`, off screen, 240 samples a minute
apart, figures after a 10 min warm-up (`results/soak4h-fd-soak.json`,
`results/soak4h-base-soak.json`, per sample `*-soak.jsonl`, logs
`results/soak4h-*.log`). Built before this round's fixes: fd from `0c8c1be`
(patch 3 as in `0ed3079`, with the terminal pane's edits of that hour), base
from `422a943`. CPU is tick-based (the cycle figure came later):

| Variant | private MB min / max / delta | CPU mean / p95, % of one core | handles | spike in front |
| --- | --- | --- | --- | --- |
| fd | 104.6 / 104.7 / -0.1 | 0.105 / 0.208 | 554 -> 550 | never |
| base | 105.2 / 105.3 / -0.1 | 1.893 / 2.812 | 554 -> 548 | never |

No growth in memory or handles over 4 hours in either. The soak is the
representative idle figure for base.

**30 minutes idle**, the soak's arguments, no `--frames` writer, built from
master `7cc2d58` and `bd94038` (`results/idle30-fd-idle.json`,
`results/idle30-base-idle.json`), % of one core:

| Variant | tick-based | cycle-based (3.7 GHz busy core) | max sample, tick / cycle |
| --- | --- | --- | --- |
| fd | 0.069 | 0.074 | 0.156 / 0.080 |
| base | 0.124 | 0.133 | 0.234 / 0.142 |

These ran while the PC was locked (the foreground was LockApp throughout), so
the display was likely off and DWM's vsync stopped driving base's per-vsync
redraws: that is why base reads 0.12% here against 1.89% in the soak. The two
figures agree within 0.01% for both, so the tick bias the reviewers expected
does not show at this level.

## Editor window

`--editor PATH` (`src/editor.rs`) opens gpui-component's code editor
(`EditorState` + `Editor`, gpui-base 0.10.0 underneath) on a Markdown file,
`.language("markdown")`, highlighted with tree-sitter-markdown
(gpui-component's `tree-sitter-markdown` feature; its C grammar compiles with
cargo-xwin's clang-cl in Luna's image as it is). The caret is steady:
`Theme::cursor_blink = CursorBlink::steady()`, so there is no blink timer.
The editor is focused with `EditorState::focus` (which starts the caret).
`samples/sample.md` is the sample; a 1 MB file for the large-file run is made
by hand into `results/big.md` (git-ignored).

Checked by unit tests (`build.ps1 -Tests`, `tests/spike.exe` on Windows,
`TestAppContext` / `VisualTestContext`): the file loads, the editor is focused
and its language is markdown; Undo and Redo (dispatched actions) undo and redo
an insert; a 1 MiB paste through the Paste action from the (test) clipboard
lands whole and undoes in one step; a steady caret gives 0 notifications of
the editor's state over 5 s of idle, while the same editor with
gpui-component's default blinking caret does notify (the control that makes
the zero mean something). The test platform's text system is not
DirectWrite, so the paste time it prints (~45 ms) says nothing about a real
window. The live paste was not tried: it would go through the desktop's
clipboard, which is the operator's.

Live (`shot.ps1 -Name editor -Title 'gpui spike editor' -IdleSeconds 10
-AppArgs '--editor,samples/sample.md'`): `results/editor.png`, highlighted;
`results/editor-big.png` on the 1.1 MB file (opened in ~0.3 s; its highlight
had not come past the first line after 5 s); `--stats` gave 0 editor
notifications while idle in both. Off screen the window is never active, so
no caret is drawn in the shots. The process's idle CPU is 1.4-2.5% of one
core, the same with the main window alone (`results/main-idle-shot.log`), so
it is not the editor; the prototype on gpui 0.2.2 idled at 0-0.5%. Not looked
into.

## Terminal pane

`src/term/` is one terminal pane on alacritty_terminal 0.26.0 (Apache-2.0;
the version the gpui-proto prototype used, resolved into this lockfile with
no other crate moving). It opens instead of the main window with `--term` /
`--pty` (a ConPTY running `pwsh -NoLogo -NoProfile`; the login shell on a Unix
pty elsewhere), `--feed burst[:SECS[xRATE]]` (a Rust port of
`test-build/perf/js/tb-feed.js`'s generator, seeded by `--seed`, on a 16 ms
tick, 4 s bursts with 1 s gaps alternating Claude Code and Codex style, every
sixth one truecolor-heavy) or `--feed-file PATH` (a recording, 16 KiB per
tick; `--record PATH` writes one). `--type TEXT` writes TEXT into its own PTY
3 s in, so a run can check the input path without any desktop input. No Zed
crate other than gpui is used, read or copied.

- `element.rs`: a gpui `Element`. Prepaint takes the Term lock once (resize
  to the cells that fit, Term and PTY; copy the viewport out with colors
  resolved), then builds background quads merged per run of equal color,
  U+2500-259F as quads and stroked paths (`boxdraw.rs`, from the characters'
  arms), and glyphs: each row shaped once with `layout_line`, font runs only
  for bold/italic, every glyph painted at its own cell in its own color
  (`paint_glyph` / `paint_emoji`). Truecolor, 256 colors, OSC 4/10/11
  overrides, bold, dim, italic, underline (single, double, curly),
  strikethrough, inverse, hidden, wide characters, a steady cursor (block,
  beam, underline; hollow when unfocused), the selection, and IME marked text
  inline at the cursor.
- `keys.rs`: xterm encoding (modifier parameter 1+shift+2·alt+4·ctrl on
  cursor/editing/F keys, SS3 under DECCKM, Ctrl/Alt characters, LNM). Plain
  printable keys are left to propagate, so they arrive as text through the
  InputHandler (WM_CHAR, IME, dead keys, AltGr). The keypad half (DECKPAM) is
  in the encoder, but gpui-pre's `Keystroke` does not tell keypad keys from
  the main ones, so no event reaches it.
- Ctrl+Shift+C / Ctrl+Shift+V (Shift+Insert) copy the selection and paste,
  through gpui's clipboard; with mode 2004 on the paste is framed by CSI 200~
  / 201~ with every ESC and ETX removed (as alacritty 0.17.0 does: some
  shells end a paste on ETX). Click-drag, double-click (word) and
  triple-click (line) select.
- `mouse.rs`: modes 1000/1002/1003, SGR (1006) or X10 encoding, each button
  tracked on its own so overlapping presses release the right one; Shift
  bypasses them. Otherwise the wheel scrolls the scrollback (as arrows in an
  alternate screen with 1007), Shift+PageUp/PageDown page it.
- IME through `EntityInputHandler`, registered in paint with
  `window.handle_input`; `bounds_for_range` returns the cursor's cell (moved
  by the composed text before the range); `text_for_range` widens a range
  that splits a surrogate pair to the whole character and reports that range.
- Parsing never happens on the UI thread: the PTY's alacritty event loop and
  the feeder thread parse into the Term behind its FairMutex and wake the UI;
  the UI holds the lock only to resize and copy the viewport. The feeders
  parse 4 KiB per acquisition of the fair lock (the UI's queue) and let go in
  between; alacritty's own PTY loop holds it for up to 64 KiB a read batch.
- The PTY is told the cells and the cell size in device pixels (exact scaled
  metrics, rounded), and again whenever either changes, a scale change
  included; ConPTY uses the cells only.
- `inactive_frame_interval: None` on its window: gpui-pre caps a window that
  is not active at 30 fps by default, and a `--background` window never is.

Measured (`stats.rs`, written when the feed ends to `--term-stats`, or to
`--stats` when no editor window is open: the editor writes that one at exit;
the JSON's `measured` says the same):

- `frame_ms`: wall time on the UI thread (`Instant`, so preemption counts; it
  is not thread CPU time) from the start of the root view's render (the pane
  is its window's root) to the end of the element's paint. Not included:
  gpui's scene submission and the GPU present.
- `latency_ms`: from the end of the parse of the oldest bytes a frame shows to
  that frame's render start. `late_frames`: frames with latency over one
  vsync (16.667 ms), i.e. output waited through a vsync with no render
  started. `output_unshown_at_end`: bytes no frame had shown 200 ms after
  the feed ended.
- `ui_lock_wait_ms` / `ui_lock_hold_ms`: the frame's one Term lock;
  `feeder_lock_*`: per 4 KiB parse chunk. `interval_ms`: render start to render
  start, when the frame's bytes were parsed within one vsync of the previous
  start. max is exact (to the microsecond), p99/p50 nearest rank.
- Bytes generated vs parsed, and FNV-1a checksums (text, and every cell's
  character, colors and flags) of scrollback + screen: the pane's Term against
  a headless Term fed the recorded bytes with the same resizes.

Results (97x29 cells, 800x500, off screen; `results/term-burst.png` is in a
truecolor burst):

- Current master (frames-on-demand and touchpad patches in), while the
  operator ran a racing game and its launcher on the same PC: `burst:60`
  (`shot.ps1 -Exe spike-term.exe -Name term-burst -WarmSeconds 26
  -IdleSeconds 40 -ExitAfter 70 -Stats results/burst.json -AppArgs
  '--feed','burst:60'`): 2994 frames; frame max 17.8 ms, p99 11.2, 2 over
  16 ms; latency max 52.0 ms, p99 9.0; 4 late frames; UI lock wait max
  0.017 ms, p99 0.000, hold max 1.6; feeder hold max 1.6, p99 0.05;
  4,580,320 bytes generated = parsed (the feeder itself stalled once, 48.8 ms
  between ticks); checksums match. `burst:30x4` (9.3 MB, `burst-x4.json`):
  frame max 34.3, p99 14.2, 7 over 16 ms; latency max 22.8, p99 8.4; 1 late
  frame; lock wait max 0.001 ms; checksums match.
- Same game load, one run each: master (`burst-rerun.json`) 20 late frames,
  latency max 35.3 ms; the unpatched vsync `spike-base.exe` with the first
  version of this pane (`burst-base.json`) 0 late frames, latency max 14.0.
  Frame max was 20.4 and 20.9 ms, one frame over 16 ms in each, so the
  frame-time outliers go with the load; the late frames only with the
  frames-on-demand build. Single noisy runs, not a conclusion.
- Before that, idle desktop, unpatched vsync, first version of this pane:
  2996 frames, frame max 14.4 ms, p99 10.7, none over 16 ms; latency max
  16.4 ms; x4 frame max 13.0, latency max 15.4; checksums match.
- `results/burst-sample.bin` (`--feed burst:6 --seed 7 --record ...`) replayed
  with `--feed-file`: the same 520,940 bytes and the same cell checksum as the
  live run (`results/burst-sample-record.json`, `results/replay.json`).
- `--pty --type '$Host.UI.RawUI.WindowSize; ...\r'`: pwsh answers 97x29, the
  pane's own grid, so the resize reached ConPTY (`results/term-pty.png`); no
  shell is left after `--exit-after`.
- Foreground: never the spike in any run (`results/term-*-shot.log`, other
  apps' titles redacted); it changed in some runs, always to the operator's
  own windows.

Tests (`build.ps1 -Tests`, `tests/spike.exe term` on Windows, 28 pass):
`TestAppContext` / `VisualTestContext` work on the Windows target. The IME
test calls `ElementInputHandler` (the object paint registers, which the IME
calls) with UTF-16 ranges and asserts marked text, selection, `text_for_range`,
`bounds_for_range` and the bytes on a fake PTY; plain typing goes through the
registered handler with `simulate_input`. The paste test puts text on the
test clipboard and dispatches Ctrl+Shift+V, the action and Shift+Insert with
2004 off and on, and ETX dropped. Key tests run through `simulate_keystrokes`
(DECCKM on/off, modifiers, Shift+PageUp scrolling); overlapping left/right
presses through `simulate_mouse_down/up` give each its own SGR release; an
IME test checks `text_for_range` on a range splitting an emoji's surrogate
pair. The encoder, mouse encoding, box drawing, colors, PTY pixel sizes at
1.5x, the stats paths, the generator and the checksum/replay have unit tests.

## macOS

`ci/gpui-spike.yml` is a GitHub workflow for a copy of this tree in Luna's
`spike/gpui` branch (`ci/COPY.md`: what to copy where, what it produces). On
macOS gpui-pre-windows and its patches are not in the graph (`cargo tree
--target aarch64-apple-darwin`, checked here), and `gpui_platform`'s
`runtime_shaders` makes the Metal shaders compile at startup, so the build
needs no Metal toolchain.

`gpui_platform` also needs its `font-kit` feature on macOS: it is what gives
gpui-pre-macos its CoreText text system, it is not a default, and without it
gpui-pre-macos uses gpui's `NoopTextSystem`, which lays text out but draws no
glyph. The first macOS run (2026-11-18) had it off: every window showed its
quads and borders but no text, and the warning gpui logs for it went unseen,
the spike having no logger then (`--log` now). The second run checks the fix
(`label_pixels` in `summary.txt`) and looks into the terminal's frame gaps
there (35 frames in 20 s, gaps up to 6 s, the feeder's 16 ms sleeps stretched
to 143 ms): ci/COPY.md, "Reading summary.txt".

## First results (phase 1, 2026-11-18)

- Release build from cold volumes: 291 s (cargo's own 3 min 19 s; the rest
  fetching the MSVC CRT/SDK into the new xwin volume). `-Features shot` after
  that: 109 s; `-Tests` (release test profile): 229 s.
- `results/spike.png`: PrintWindow shot of the window off screen at x=3632
  (right of every display), exstyle `0x08200180` (`WS_EX_NOACTIVATE |
  WS_EX_NOREDIRECTIONBITMAP | WS_EX_WINDOWEDGE | WS_EX_TOOLWINDOW`), first
  window 0.8-0.9 s after launch, foreground unchanged before/after and never
  the spike (`results/spike-shot.log`).
- `tests/spike.exe` (from `-Tests`) runs on Windows: 2 passed.

## Build

`pwsh ./build.ps1` (PowerShell 7.4+, Docker): Luna's Windows image through the
repo's `scripts/lib.ps1` (`docker/windows-build.Dockerfile`), extended by
`docker/Dockerfile` (the cache path only), then `docker/build.sh` runs
`cargo xwin build --release --locked --target x86_64-pc-windows-msvc` and
copies `spike.exe` here. `pwsh ./build.ps1 -Tests` runs
`cargo xwin test --release --no-run` and copies the test executables to
`tests/` (they run on Windows). `pwsh ./build.ps1 -Base` builds the branch
`base-vsync` (this tree before the frames-on-demand patch, commit `422a943`)
from its worktree `.wt/base` into `spike-base.exe` (and `tests-base/` with
`-Tests`), with a target dir of its own in the volume. Volumes: `gpui-spike-target`,
`gpui-spike-xwin-cache` (its own) and Luna's `luna-cargo-registry`. Remove its
own with `docker volume rm gpui-spike-target gpui-spike-xwin-cache`.

`Cargo.lock` was seeded from the `Cargo.lock` that gpui-component 0.10.0
ships, so third-party crates resolve as that snapshot was tested.

## Versions

- `gpui-component` =0.10.0, which pins `gpui-pre` =0.3.12 (and
  `gpui-pre-macros`, `gpui-pre-sum-tree` =0.3.12).
- Rust 1.100.0 and cargo-xwin 0.23.1, from Luna's image.
- `gpui-pre-platform` =0.3.12 (`gpui_platform::application()`), which pulls in
  `gpui-pre-windows` =0.3.12 on Windows.

## Patches

One crate is patched, `gpui-pre-windows` (Apache-2.0), vendored in
`vendor/gpui-pre-windows` through `[patch.crates-io]`. The first commit of this
repo is the crate as published; every change since is marked
`gpui-spike patch` (`git grep "gpui-spike patch"`): 5 files, 12 markers, three
patches. (The branch `base-vsync`, master without patch 3, has the first two
only: 2 files, 4 markers; the tag `base-vsync-422a943` is the older base it
replaced, without the terminal pane's latest code.)

**1. Shaders at runtime** — `src/directx_renderer.rs`, 1 marker. Upstream's
release build includes bytecode that `build.rs` makes with `fxc.exe`, and only
when the *build host* is Windows (its `cfg` is the host's), so a Linux
cross-build of a release has no `shaders_bytes.rs` and fails; its debug build
compiles the `.hlsl` at runtime from the build host's path. The patch replaces
the `shader_resources` module: the HLSL is embedded (`include_str!`) and
compiled at startup with `D3DCompile` (`d3dcompiler_47.dll`, shipped with
every Windows 10/11, the compiler `fxc.exe` wraps), same profiles and `/O3`.
`build.rs` needs no patch: on a Linux host it does nothing. gpui-pre-platform's
`runtime_shaders` feature does not help: it only forwards to `gpui_macos` and
`gpui_ios`. *Upstreamable?* As an opt-in, plausibly: macOS already has
`runtime_shaders` for the same reason, so a Windows arm of that feature fits
upstream's own pattern; as the default, unlikely, since it costs a compile at
every start (part of the ~0.8-1 s to a first window here, not timed alone).

**2. Unattended windows** — `src/window.rs`, 3 markers. Upstream already shows
a window opened with `focus: false` with `SW_SHOWNOACTIVATE`, and gpui itself
never calls `activate()` on open. Patched, for a window opened with
`focus: false`: it is `WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE` instead of
`WS_EX_APPWINDOW` (off the taskbar and Alt+Tab, never activated by a click);
it keeps the bounds it was given even off every display (upstream moves them
to the primary's defaults); and `activate()` does nothing for a
`WS_EX_NOACTIVATE` window (upstream's sends a synthesized Alt keypress with
`SendInput` to win the foreground; gpui-component's notifications call it).
*Upstreamable?* Not as it is: it gives `focus: false` a meaning (no taskbar,
no activation, any bounds) that upstream does not, and an ordinary app opening
a window unfocused would lose its taskbar button. As an explicit window kind
or option for background/tool windows it would be a reasonable proposal, and
the `activate()` guard on its own is a plain fix.

Left as they are, since the off-screen mode cannot reach them:
`events.rs` answers `WM_MOUSEACTIVATE` with `MA_ACTIVATE` always, which would
activate a `WS_EX_NOACTIVATE` window on a click, but a window on no display
cannot be clicked; and `PlatformWindow::resize` (`window.rs`) calls
`SetWindowPos` without `SWP_NOACTIVATE`, but nothing in the spike,
gpui-component or gpui-base calls `Window::resize` (term-check.ps1 resizes the
window with its own `SetWindowPos(SWP_NOACTIVATE | ...)`). A no-activate
window placed on a display would need both.

The spike puts such a window past the right edge of the virtual screen in
physical pixels, divided by the primary display's scale, the one this crate
converts the origin with (`src/win.rs`; the spike's own code, not a patch):
taking the largest logical right edge of the displays, as it first did, lands
the window on a secondary display scaled more than the primary (each
display's logical bounds are divided by its own scale). Every script that
starts the spike (shot, term-check, measure-idle, soak, ab-burst) fails if any
window of the spike touches a display (`MonitorFromRect` with
`MONITOR_DEFAULTTONULL`). Tried on this PC's displays only, which share one
scale: a mixed-DPI setup is untested.

**3. Frames on demand** — `src/window.rs` 2 markers, `src/platform.rs` 3,
`src/events.rs` 2, `src/direct_manipulation.rs` 1. Upstream's vsync thread
`RedrawWindow`s every visible window on every vsync (164 times a second
here), so an idle window's main thread wakes that often to find nothing to
draw: 1.2-2% of a core per process idle (README "Idle cost"). gpui core
already supports demand-driven platforms (`PlatformWindow::frame_waker`,
called when a window becomes dirty, and `schedule_frame`, for follow-up
frames); gpui-pre-windows leaves both at their no-op defaults. The patch:

- A request is a property on the window's own HWND plus a signal of one
  auto-reset event (`window.rs`). The vsync thread sleeps on the event with no
  timeout, redraws at once the windows whose property it takes, then waits for
  a vsync only to pace the next round (`platform.rs`). Signals that coalesce
  still find every property, and a request made during a scan leaves the event
  signaled for the next round. Stopping signals the event too (`detach_gpu`),
  and a thread that wakes to stop signals it again on its way out, so after a
  detach and a reattach a request the old thread took goes to the new one.
- `draw_window` (`events.rs`) validates the update region before the draw,
  not after it as upstream does: the vsync thread can take a request gpui makes
  during the draw and invalidate the window before the draw returns, and
  validating afterwards would drop that frame. A draw deferred because another
  was in progress asks for its frame again (gpui, still dirty, will not). A
  draw that presented nothing (counted in `PlatformWindow::draw`) asks again,
  up to 8 times in a row, which covers gpui core's own early return for a
  request that comes during one of its draws (a deferred forced render after a
  device loss among them); upstream's redraw on every vsync covered those.
- DirectManipulation (touchpad gestures) is pumped only in a drawn frame, so
  a touchpad contact asks for a frame, and so does every pump for 250 ms after
  it and while the gesture or its inertia runs (`direct_manipulation.rs`).

A failed resize that invalidates the devices needs nothing of its own: the
resize dirties its window, which asks for a frame, and the vsync thread checks
the devices first. *Upstreamable?* Yes in substance: it is the Windows side of
an interface gpui already has, and Zed's master still redraws every window on
every vsync. Upstream would want a per-window atomic in its tracked window
list instead of the window property, and review of the input paths that only
the redraw on every vsync used to serve. Untested here: a real touchpad,
high-rate input, device loss, mixed DPI.

## Notes

- `render_to_image` exists on Windows in gpui-pre-windows 0.3.12
  (`Window::render_to_image`), but only behind the `test-support` feature;
  `shot.ps1` uses PrintWindow from outside the process instead, so the release
  exe stays as shipped.
- No Zed GPL crate is used, read or copied.

## Idle cost

`measure-idle.ps1` samples the spike's own process tree (CPU % of one core,
memory, handles, threads, its busiest threads by name, the app's render
counters through `--frames`, the foreground window) and writes
`results/<Name>-idle.json`; `soak.ps1` does the same for hours, appending to
`results/<Name>-soak.jsonl`. Both share `sampler.ps1`. `--bare` opens a plain
GPUI window without gpui-component (`src/idle.rs`); `--tick SECS` makes it
change every SECS, to see a repaint on demand.

Measured 2026-11-18 on this PC (a 164 Hz display), 30 s after a 10 s warm-up,
% of one core for spike.exe (CPU time comes in 15.6 ms ticks, so 0.05% is one
tick over 30 s; single runs, ±0.5% between runs unpatched):

| Configuration | Unpatched (`spike-base.exe`) | Frames on demand (`spike.exe`) |
| --- | --- | --- |
| bare GPUI window | 1.96 | 0.15 |
| gpui-component root (default window) | 1.24 | 0.05 |
| + editor window | 1.39 | 0.10 |
| terminal (`--term --pty`) + editor | 1.75 (tree 2.01 with pwsh) | 0.21 (tree 0.31) |

Nothing renders while idle (0 renders/s in every row), so the cost is not
gpui-component, the caret or drawing: it is gpui-pre-windows' vsync loop.
Its `VSyncProvider` thread (`vendor/gpui-pre-windows/src/platform.rs`,
`begin_vsync_thread`) wakes on every vsync and `RedrawWindow`s every visible
window, so each window's main thread gets a WM_PAINT and runs gpui's frame
request 164 times a second only to find nothing dirty. gpui itself can drive
frames on demand (`PlatformWindow::frame_waker` / `schedule_frame`), which
gpui-pre-windows leaves at their no-op defaults. Patch 3 (README "Patches")
implements them; the frames-on-demand column above was measured with it, then
uncommitted, before it went into master (commit `0ed3079`). With it a
`--tick 0.5` bare window still renders 2 frames/s and shows them, and
term-check.ps1 on it (`results/termcheck-fd.log`): the 60 s burst ran 3002
frames (reference `results/burst.json`, unpatched: 2996), frame ms max 12.8 /
p99 10.5 (14.4 / 10.7), latency ms max 8.2 / p99 6.4 (16.4 / 15.1), checksum
and bytes match; three resizes of its own window during a 20 s burst left a
144x42 grid with the checksum matching; under `--pty`, a resize to a 984x596
client area before a typed command had pwsh report a 119x35 console
(`results/termcheck-fd-pty-pty-size.txt`, shot `termcheck-fd-pty-pty.png`),
so the PTY follows. The editor and main windows render as before
(`results/fd-*.png`) and the unit tests pass. Untested: device loss, a real
touchpad, high-rate input.

The sampler counts a process from the first snapshot it is in: a child that
starts and exits between two samples is not counted at all. The spike's own
figure is its process alone, children never included.

Two 4-hour soaks (`--term --pty --editor`, off screen), unpatched and patched
side by side, write `results/soak4h-base-soak.jsonl` and
`results/soak4h-fd-soak.jsonl` (summaries `results/soak4h-*-soak.json`, logs
`results/soak4h-*.log`); they run copies of the two exes from `.soak/`.

### Burst under load, A/B

`ab-burst.ps1` runs each variant on `--term --feed burst:60 --term-stats`,
interleaved (a, b, c, then c, b, a, ...) so desktop load falls on all alike,
and samples the desktop's total CPU and its top consumers (names only).
2026-11-18, 4 rounds, with a game running on this PC (desktop CPU 80-97%):
`base` = `spike-base.exe` (`base-vsync`, unpatched), `fd` = `spike.exe`
(`deec691`), `prefix` = master before that fix (`c05bff5`). Per run in
`results/ab/fix1.log` and `results/ab/fix1.json`; medians:

| Variant | late frames | latency max ms | latency p99 ms | frame max ms | frame p99 ms | checksum | desktop CPU % |
| --- | --- | --- | --- | --- | --- | --- | --- |
| base | 1 | 44.7 | 14.1 | 16.3 | 10.8 | 4/4 | 93.6 |
| fd | 1.5 | 24.6 | 10.0 | 17.1 | 10.8 | 4/4 | 88.3 |
| prefix | 0.5 | 17.9 | 11.7 | 14.3 | 10.8 | 4/4 | 90.6 |

Under that load every variant has some late frames (base 0-3 per run, with
one 373 ms stall), and the three do not separate at 4 runs each: the 20 late
frames against 0 of a single earlier pair did not come back.

### Measuring honestly

- CPU: `sampler.ps1` gives two figures. `cpuPct` is TotalProcessorTime, which
  Windows charges by timer tick (15.6 ms), so short wakes go uncharged and an
  idle process reads low; `cyclePct` is QueryProcessCycleTime (and
  QueryThreadCycleTime per thread), charged exactly, turned into % of one core
  with the cycles a busy core runs per second, measured once per script by
  spinning the sampler's own thread for 1 s (clock speeds vary, so it is an
  estimate of "one core busy"). `measure-idle.ps1 -NoFrames` leaves out the
  app's 1 Hz counter writer.
- Foreground: every script runs a watcher (`SetWinEventHook`,
  `EVENT_SYSTEM_FOREGROUND`, out of context, on a thread of the script's own
  process: passive, nothing injected) and fails if the spike's pid ever came
  to the foreground. Other applications are logged by exe name only.
- The terminal's summary: `checksum` compares the pane's Term with a headless
  Term fed the same recorded bytes with the same resizes, so it checks resize
  handling and the final state (scrollback limited to the last 10000 lines),
  not what each frame showed; `bytes.equal` holds by construction of the feed.
  `output_unshown_at_end` is the signal that nothing was held back. Latency
  ends at a frame's render start, not at its present. `late_frames` counts
  latencies over one 60 Hz vsync (16.7 ms), `late_frames_display_vsync` over
  one vsync of the primary display (`display_hz`, 164 Hz here: 6.1 ms).
