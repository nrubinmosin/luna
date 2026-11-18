# Vendored crates

`gpui-pre-windows` 0.3.12 (Apache-2.0), GPUI's Windows platform crate, is
vendored here and patched in through `[patch.crates-io]` in
`src-tauri/Cargo.toml`. Its first commit in this repository is the crate exactly
as its `.crate` file on crates.io holds it; every change since is Luna's and is
marked `luna patch` (`git grep "luna patch" -- src-tauri/vendor`): 5 files, 12
markers, three patches. The files are kept byte for byte (`-text` in
`.gitattributes`), so `git diff <first commit> -- src-tauri/vendor` is the
patches and nothing else.

The patches were made and measured in a spike before they came here (frames,
latency and idle CPU on this crate with and without them); they changed
nothing but their marker and the frame-demand property's name on the way.

On macOS and Linux the crate is not in the graph at all.

A fourth patch, or a new snapshot of gpui-pre, is a decision of its own:
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

`src/window.rs`, 3 markers. Upstream already shows a window opened with
`focus: false` with `SW_SHOWNOACTIVATE`, and gpui itself never calls
`activate()` on open. Patched, for a window opened with `focus: false`: it is
`WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE` instead of `WS_EX_APPWINDOW` (off the
taskbar and Alt+Tab, never activated by a click); it keeps the bounds it was
given even off every display (upstream moves them to the primary's defaults);
and `activate()` does nothing for a `WS_EX_NOACTIVATE` window (upstream's sends
a synthesized Alt keypress with `SendInput` to win the foreground;
gpui-component's notifications call it).

Luna opens its window so only in the test build's background mode
(`LUNA_TEST_WINDOW=background`): the window goes past the right edge of every
display (`crates/window/src/offscreen.rs`), never activated, never on the
taskbar. Left as they are, since that mode cannot reach them: `events.rs`
answers `WM_MOUSEACTIVATE` with `MA_ACTIVATE` always, and
`PlatformWindow::resize` calls `SetWindowPos` without `SWP_NOACTIVATE`. A
no-activate window placed on a display would need both.

## 3. Frames on demand

`src/window.rs` 2 markers, `src/platform.rs` 3, `src/events.rs` 2,
`src/direct_manipulation.rs` 1. Upstream's vsync thread `RedrawWindow`s every
visible window on every vsync, so an idle window's main thread wakes that
often to find nothing to draw: 1.2-2% of a core per process idle. gpui core
already supports demand-driven platforms (`PlatformWindow::frame_waker`, called
when a window becomes dirty, and `schedule_frame`, for follow-up frames);
gpui-pre-windows leaves both at their no-op defaults. The patch:

- A request is a property on the window's own HWND (`luna-frame-demand`) plus a
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
- DirectManipulation (touchpad gestures) is pumped only in a drawn frame, so a
  touchpad contact asks for a frame, and so does every pump for 250 ms after it
  and while the gesture or its inertia runs (`direct_manipulation.rs`).

Untested: a real touchpad, high-rate input, device loss, mixed DPI.
