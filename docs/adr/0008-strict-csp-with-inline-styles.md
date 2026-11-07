# ADR-0008: A strict Content Security Policy, with inline styles allowed

- **Status:** Accepted
- **Date:** 2026-11-07 (`1cb24c4`, refined in `3d787a6`)

## Context

The app shipped with `csp: null`. The page renders untrusted text (terminal
output, transcripts, file names), and any script that ran in it could invoke
every Tauri command (finding H3; the unscoped commands are F9 in the
[threat model](../security/threat-model.md)).

## Decision

`src-tauri/tauri.conf.json` sets `default-src 'self'`, `script-src 'self'` (no
inline script, no `eval`), `connect-src ipc: http://ipc.localhost`,
`img-src 'self' data:`, `font-src 'self'`, `object-src 'none'`,
`base-uri 'self'`, `form-action 'none'`, `frame-ancestors 'none'`. Refused
loads are logged as `[csp]` warnings (`src/shared/lib/log.ts`).

`style-src` keeps `'unsafe-inline'`: the pinned xterm.js 5.5.0 creates
`<style>` elements and has no nonce option, and a nonce would make the browser
ignore `'unsafe-inline'`, so Tauri's style nonce is off
(`dangerousDisableAssetCspModification: ["style-src"]`). `devCsp` (only
`pnpm tauri dev`) adds `'unsafe-inline'` scripts for the Vite preamble and
`ws://localhost:1420` for HMR.

## Consequences

- Injected markup can restyle the page but runs no script.
- The allowance is owed: revisit when xterm.js offers a nonce, or the DOM
  renderer is no longer needed.
- On Linux and macOS Tauri delivers the policy as a `<meta>` tag, where
  `frame-ancestors` is ignored; only Windows was tested live.
- Command scoping (F9) remains the stronger fix and is still open.
