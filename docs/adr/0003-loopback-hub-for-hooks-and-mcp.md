# ADR-0003: One loopback HTTP hub for hooks, the guard and MCP

- **Status:** Accepted
- **Date:** 2026-09-23 (hooks, `a33763a`, for keep-awake); MCP joined it on
  2026-09-24 (`3efaab9`)

## Context

Luna needs three things from a running CLI: turn boundaries (Claude Code's
hooks), a veto on certain shell commands before they run (the guard), and a
channel for agents to call Luna's tools (MCP). Both CLIs speak HTTP for MCP and
can run a hook command that posts JSON. Alternatives were named pipes or Unix
sockets per session (platform-specific, and not what the CLIs speak), stdio MCP
servers (one process per session, no shared registry), or tailing files
(latency, no request/response).

## Decision

One small HTTP/1.1 server of Luna's own on `127.0.0.1` and a random port
(`hub.rs`), one request per connection:

- `POST /hook/<chat>/<secret>` takes a Claude Code hook's stdin.
- `POST /guard/<chat>/<secret>` answers the PreToolUse guard (`guard.rs`).
- `POST /mcp` with the session's bearer token is the MCP server (`mcp.rs`).

A chat's secret is an HMAC of its id under a per-launch key (`secret.rs`); MCP
tokens are per session, reissued on every spawn, and travel in the environment
(`LUNA_MCP_TOKEN`), never in a file. Requests are parsed with `httparse`, run
on a thread each in bounded lanes (hooks apart from MCP calls, `admit.rs`),
with deadlines for the whole request.

## Consequences

- One listener serves both CLIs and every session, and lives in the core
  ([ADR-0002](0002-core-process-outlives-the-window.md)), so `luna_wait`
  works with no window open.
- Hooks print nothing, so nothing reaches the model's context; with the hub
  down a hook or the guard fails open (the call goes ahead).
- Loopback is reachable by any local process and by browsers' simple
  cross-origin requests: authentication happens on every route, and limits
  apply before it. See B3 in the [threat model](../security/threat-model.md).
- Long MCP calls (`luna_wait` holds a request up to 30 minutes) need each
  CLI's client timeout raised ([cli-compatibility](../cli-compatibility.md)).
