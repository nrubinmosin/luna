# Vendored crates

`gpui-pre-windows` 0.3.12 (Apache-2.0), GPUI's Windows platform crate, is
vendored here and patched in through `[patch.crates-io]` in
`src-tauri/Cargo.toml`. Its first commit in this repository is the crate exactly
as its `.crate` file on crates.io holds it; every change since is Luna's and is
marked `luna patch` (`git grep "luna patch" -- src-tauri/vendor`): 6 files, 21
markers, four patches. The files are kept byte for byte (`-text` in
`.gitattributes`), so `git diff <first commit> -- src-tauri/vendor` is the
patches and nothing else.

The first three patches were made and measured in a spike before they came
here (frames, latency and idle CPU on this crate with and without them); they
changed nothing but their marker and the frame-demand property's name on the
way. The fourth came with 0.20.1.

On macOS and Linux the crate is not in the graph at all.

A further patch, or a new snapshot of gpui-pre, is a decision of its own:
re-vendor the new crate as published in one commit, then carry the patches
over in the next.

## 1. Shaders at runtime

`src/directx_renderer.rs`, 1 marker. Upstream's release build includes
bytecode that `build.rs` makes with `fxc.exe`, and only when the *build host*
is Windows (its `cfg` is the host's), so a Linux cross-build of a release has
no `shaders_bytes.rs` and fails; its debug build compiles the `.hlsl` at
runtime from the build host's path. The patch replaces the `shader_resources`
module: the HLSL is embedded (`include_str!`) and compiled at startup with
`D3DCompile` (`d3dcompiler_47.dll`, shipped with every Windows 10/11, the
compiler `fxc.exe` wraps), same profiles and `/O3`. `build.rs` needs no patch:
on a Linux host it does nothing. gpui-pre-platform's `runtime_shaders` feature
does not help: it only forwards to the macOS and iOS crates.

## 2. Unattended windows

`src/window.rs` 4 markers, `src/events.rs` 1, `src/gpui_windows.rs` 1.
Upstream already shows a window opened with `focus: false` with
`SW_SHOWNOACTIVATE`, and gpui itself never calls `activate()` on open. The
patch adds a process-wide switch, `gpui_windows::set_unattended_windows`, off
until a caller turns it on. With it on, a window opened with `focus: false` is
unattended: it is `WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE` instead of
`WS_EX_APPWINDOW` (off the taskbar and Alt+Tab); it keeps the bounds it was
given even off every display (upstream moves them to the primary's
defaults), and is shown there with `SetWindowPos(SWP_NOACTIVATE |
SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_SHOWWINDOW)` instead of
`SetWindowPlacement` (whose rectangle is in workspace coordinates, not the
screen's); `WM_MOUSEACTIVATE` is answered `MA_NOACTIVATE` (upstream answers
`MA_ACTIVATE` always), so a click does not activate it; and `activate()` does
nothing for it (upstream's sends a synthesized Alt keypress with `SendInput`
to win the foreground; gpui-component's notifications call it). With the
switch off, `focus: false` means what it does upstream.

Luna turns the switch on only in the test build's background mode
(`LUNA_TEST_WINDOW=background`, `crates/window/src/lib.rs` `run`) and in the
shots harness: the window goes past the right edge of every display
(`crates/window/src/offscreen.rs`), never activated, never on the taskbar.
Left as it is, since nothing in that mode resizes the window:
`PlatformWindow::resize` calls `SetWindowPos` without `SWP_NOACTIVATE`.

## 3. Frames on demand

`src/window.rs` 2 markers, `src/platform.rs` 3, `src/events.rs` 4,
`src/direct_manipulation.rs` 1. Upstream's vsync thread `RedrawWindow`s every
visible window on every vsync, so an idle window's main thread wakes that
often to find nothing to draw: 1.2-2% of a core per process idle. gpui core
already supports demand-driven platforms (`PlatformWindow::frame_waker`, called
when a window becomes dirty, and `schedule_frame`, for follow-up frames);
gpui-pre-windows leaves both at their no-op defaults. The patch:

- A request is a property on the window's own HWND (`rm-luna-frame-demand`) plus a
  signal of one auto-reset event (`window.rs`). The vsync thread sleeps on the
  event with no timeout, redraws at once the windows whose property it takes,
  then waits for a vsync only to pace the next round (`platform.rs`). Signals
  that coalesce still find every property, and a request made during a scan
  leaves the event signaled for the next round. Stopping signals the event too
  (`detach_gpu`), and a thread that wakes to stop signals it again on its way
  out.
- `draw_window` (`events.rs`) validates the update region before the draw, not
  after it as upstream does: the vsync thread can take a request gpui makes
  during the draw and invalidate the window before the draw returns, and
  validating afterwards would drop that frame. A draw deferred because another
  was in progress asks for its frame again. A draw that presented nothing asks
  again, up to 8 times in a row, which covers gpui core's own early return for
  a request that comes during one of its draws.
- Only a present that succeeded counts as one (`window.rs` `draw`). One that
  failed (the device removed or reset after the vsync thread last looked)
  wakes the vsync thread, which checks the device and recovers it, and the
  draw asks for its frame again; asleep, the thread would not have looked
  before some unrelated request. A failed resize of the renderer, which sets
  the flag that has the devices recreated, wakes it too (`events.rs`).
- A window restored from minimized asks for a forced frame (`events.rs`
  `handle_size_msg`): a request made while it was minimized found no callback
  to draw with and was spent, and gpui, no longer dirty, asks for none.
- DirectManipulation (touchpad gestures) is pumped only in a drawn frame, so a
  touchpad contact asks for a frame, and so does every pump for 250 ms after it
  and while the gesture or its inertia runs (`direct_manipulation.rs`).

Untested: a real touchpad, high-rate input, device loss, mixed DPI.

## 4. Frames through input

`src/window.rs` 2 markers, `src/events.rs` 2. With frames on demand (patch 3)
a window presents only when something changed. A variable-refresh monitor
(G-SYNC, FreeSync) whose refresh follows the window's presents then falls to
its floor between them (48 Hz on the 48-165 Hz monitor it was seen on), and
everything on that monitor stutters while Luna is used: a key or a scroll
waits for the next slow refresh. Upstream never meets this, as its vsync
thread redraws every window on every vsync; gpui itself keeps presenting for
a second after high-rate input (its `InputRateTracker`, zed-industries/zed
PR 45369, Apache-2.0), which under frames on demand nothing asks for. The
patch is the same idea for the person's own input, not ported code:

- The person's own input in a window (`counts_as_input`: keys, text and IME
  composition, mouse buttons, wheels, a move with a button held; not a bare
  mouse move) keeps it alive for a second (`KeepAlive`, `KEEP_ALIVE`),
  restarted by each input (`events.rs` `handle_msg`), and asks for a frame.
- A draw of a window kept alive passes `require_presentation` to gpui, which
  then presents even an unchanged frame and skips its throttling, and asks
  for the next vsync's frame (`events.rs` `draw_window`). So the window
  presents at the display's rate until a second after the last input, then
  stops; an idle window draws nothing, as before.

For NVIDIA's G-SYNC Luna also turns G-SYNC off for its own exe in the
driver's profiles (src/desktop/gsync.rs), which leaves the monitor at its
fixed rate whatever Luna presents; this patch is for the variable refresh
that setting does not reach (another vendor's, or a driver that refused it).

The rule's unit tests are in the crate (`keep_alive_tests`): the vendored
crate is outside the workspace, so CI does not run them, and
`cargo xwin test -p gpui-pre-windows --lib keep_alive_tests --target
x86_64-pc-windows-msvc` in the Windows build image, with the executable run
on Windows, does. Not measured on a VRR monitor: the test build runs off
screen.
