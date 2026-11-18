# Copying the spike into Luna's `spike/gpui` branch

The spike lives in `test-build/gpui-spike/` (git-ignored, its own local repo).
For the macOS run it is copied into a branch `spike/gpui` off Luna's master:
the tree to `spike/gpui/`, the workflow to `.github/workflows/`.

## Files

Copy as they are in the spike's latest commit on `master` (`git -C
test-build/gpui-spike ls-files` lists them), relative to
`test-build/gpui-spike/`. `master` is the patched variant (README "Patches":
5 files, 12 markers); the unpatched `base-vsync` branch stays in the spike's
own repo, so `build.ps1 -Base` does not work in the copy.

| From | To |
| --- | --- |
| `Cargo.toml`, `Cargo.lock` | `spike/gpui/` |
| `src/` (all of it: `main.rs`, `editor.rs`, `idle.rs`, `win.rs`, `term/`) | `spike/gpui/src/` |
| `vendor/gpui-pre-windows/` (all of it) | `spike/gpui/vendor/gpui-pre-windows/` |
| `samples/` | `spike/gpui/samples/` |
| `docker/Dockerfile`, `docker/build.sh` | `spike/gpui/docker/` |
| `build.ps1`, `shot.ps1`, `term-check.ps1`, `measure-idle.ps1`, `soak.ps1`, `sampler.ps1`, `ab-burst.ps1`, `after-soak.ps1` | `spike/gpui/` |
| `results/burst.json` (term-check.ps1's reference run; nothing else from `results/`) | `spike/gpui/results/` |
| `README.md`, `.gitignore`, `.gitattributes` | `spike/gpui/` |
| `ci/COPY.md` | `spike/gpui/ci/` |
| `ci/gpui-spike.yml` | `.github/workflows/gpui-spike.yml` (not under `spike/gpui/`) |

`build.ps1` reaches Luna's `scripts/lib.ps1` as `../../scripts/lib.ps1`,
which from `spike/gpui/` is the repo's own, as from `test-build/gpui-spike/`.

From pwsh at the Luna checkout's root, on the `spike/gpui` branch:

```
$from = 'test-build/gpui-spike'
New-Item -ItemType Directory -Force spike/gpui, .github/workflows | Out-Null
git -C $from ls-files | Where-Object { $_ -eq 'results/burst.json' -or $_ -notmatch '^(results/|ci/gpui-spike\.yml$)' } | ForEach-Object {
    $dest = Join-Path spike/gpui $_
    New-Item -ItemType Directory -Force (Split-Path $dest) | Out-Null
    Copy-Item (Join-Path $from $_) $dest
}
Copy-Item "$from/ci/gpui-spike.yml" .github/workflows/gpui-spike.yml
```

## Leave out

- `spike.exe`, `spike-*.exe` (build outputs, `spike-base.exe` among them),
  `tests/`, `tests-base/` (test executables), `target/`.
- `.wt/` (the `base-vsync` worktree) and `.soak/` (exe copies a soak runs).
- `results/` but `burst.json`: screenshots, logs and stats of local runs
  (`results/big.md` is generated and git-ignored anyway).
- `.crates/`: crate sources downloaded for reading.
- `.git/`: the spike's own history stays in `test-build/gpui-spike/`.

## What the job produces

`.github/workflows/gpui-spike.yml` runs on a push to `spike/gpui` and on
`workflow_dispatch`, on `macos-15` (Apple silicon), Rust 1.100.0, in
`spike/gpui/`. One artifact, `gpui-spike-macos`:

- `versions.txt`: macOS, Xcode, rustc and cargo versions.
- `cargo-tree.txt`: the macOS dependency graph. The step fails if the
  Windows-only `gpui-pre-windows` (the patched, vendored crate) shows up in it.
- `build.txt`: release build seconds, binary size, the `shot` build's seconds.
- `tests.txt`: `cargo test --release` output (the editor's undo/redo,
  1 MB paste and steady-caret tests among them).
- `run.txt`, `term-stdout.txt`, `term-stderr.txt`, `term-stats.json`: the
  terminal pane on `--feed burst:20`, quitting itself at 30 s;
  `term-stats.json` is its feed summary (frames, frame ms, latency, the
  checksum against a headless Term).
- `editor-stdout.txt`, `editor-stderr.txt`, `editor-stats.json`: the editor
  window on `samples/sample.md`, quitting itself at 15 s; its `--stats` line
  (bytes, open time, notifications while idle).
- `term-desktop.png`, `editor-desktop.png`: `screencapture -x` of the
  runner's desktop during each run.
- The "Check the stats" step fails the job when either stats file is missing
  or unreadable, the terminal's checksum or byte count does not match, it drew
  no frames, the editor loaded no text, or the idle editor notified.
- `inapp.png`, `inapp-editor.png`, `inapp.txt`: the main and editor windows
  rendered in-app with `Window::render_to_image` (a second build with
  `--features shot`; the step may fail without failing the job).

## Other workflows a push to `spike/gpui` starts

Of Luna's workflows only `ci.yml` (`push: branches: ['**']`) also runs on
that push: its frontend job (with the commit-message check over the pushed
commits, and `pnpm check:text`), `rust-linux`, `windows-build` and
`windows-tests`. `release.yml` (tags `v*`), `security.yml` (master, PRs,
schedule) and `fuzz.yml` (schedule) do not.

Expect `ci.yml`'s frontend job to fail there: `pnpm check:text` checks every
tracked file, and Luna's `scripts/check-text.mjs` run over the copied files
(2026-11-18, master `3f52a3a` plus this file) finds 48 lines: 19 in the
vendored Zed code (`vendor/gpui-pre-windows/src/`: `dialog.rs`,
`direct_write.rs`, `directx_renderer.rs`, `platform.rs`; British spellings in
comments and non-Latin test strings) and 29 in `src/term/tests.rs` (the
terminal pane's wide-character tests, CJK text on purpose). Either let that
job fail on the branch, or mark those lines `lang-check: allow` in the copy
(the vendored ones are an edit to Zed's code, though not a behavior patch).
