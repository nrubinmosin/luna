# Working in this repo

Rules for anyone (person or agent) changing Luna.

## Where things go
- The repo holds only what is in use. No scratch files, experiments or one-off
  scripts get committed; delete what a change made obsolete (icons, CSS,
  commands, assets) in the same change.
- Never create folders next to the repo or elsewhere on disk for testing
  (no `E:\Projects\Personal\luna-test` and the like). Temporary files go in
  your scratchpad or in `test-build/`.

## Building and testing
- There is no local Rust or Node toolchain: everything builds in Docker.
  - `./build-windows.ps1` — the release exe (`-Installer` for the setup).
  - `./test-build.ps1` — the **test build**, `test-build/luna-test.exe`.
  - `./screenshots.ps1 -Out test-build/shots` — headless shots of the UI on
    the demo data in `src/dev/demo.ts`.
- Test only on the test build, never on the Luna in use. It has its own
  identifier and keeps its log, attachments, settings and downloaded CLIs
  beside its exe in `test-build/` (ignored by git), so it runs alongside the
  real one. `./test-build.ps1 -Run -DevCaller ...` also registers a scripted
  MCP caller for exercising Luna tools.
- Anything that writes into account folders (settings, sync) is tried on
  throwaway accounts under `test-build/accounts`, not on real ones.

## Code
- Match the surrounding code: its comment density (comments say why), naming
  and idioms. Comments describe what is there now, not what used to be.
- Cross-platform: Windows is home, but nothing may assume it — gate
  Windows-only behaviour and defaults (e.g. `defaultShell: powershell`).
