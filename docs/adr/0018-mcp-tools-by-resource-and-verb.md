# ADR-0018: Name the MCP tools by resource and verb, and break the old names

- **Status:** Accepted
- **Date:** 2026-11-09

## Context

Luna's agent tools grew one at a time to twelve (`luna_list`, `luna_sessions`,
`luna_spawn`, `luna_send`, `luna_answer`, `luna_read`, `luna_wait`, `luna_kill`,
`luna_delete`, `luna_guide`, `luna_notify`, `luna_power`). Two independent audits
(2026-11-08) found the surface costly and ambiguous: `luna_list` mixed sessions
and accounts and returned every field of every row; `luna_sessions` returned
every session at some 800 characters a row; `luna_wait` returned everything a
helper said, up to 60,000 characters, with a screen by default; the instructions
told every caller to read the whole 16 KB guide before its first spawn; argument
names mixed units (`timeoutS`, `deletesInMin`) and meanings (`tools` granted
tools on a spawn and marked tool calls on a read). The owner asked for
REST-like names, list and get per resource and verbs for actions, the old names
removed outright with no shims.

## Decision

Sixteen tools named `luna_<resource>_<verb>` (`mcp/tools.rs`): sessions
(`list`, `get`, `spawn`, `send`, `answer`, `wait`, `stop`, `delete`), messages
(`list`), reports (`get`), accounts (`list`, `get`), runs (`list`), the guide
(`get`), notifications (`send`) and power (`set`). A `list` is compact rows a
page at a time, filtered on the registry before anything costly is read; a `get`
is one item whole. A wait returns the report by default. Units say themselves
(`…Sec`, `…Min`). The guide is read by a fixed topic per tool plus `notices`,
`restart` and `helpers` (`mcp/guide.rs`), and the instructions name the two
topics to read first. Calling an old name (`REMOVED`) is an error listing the
current tools; nothing answers under an old name.

## Consequences

- A tooled session's tool list grows by about 250 tokens (four more tools), and
  its polls, waits and listings cost a fraction of what they did; nobody reads
  the whole guide.
- Every agent instruction, skill and prompt naming an old tool breaks until it is
  updated: the owner's library files are updated separately. A session running
  across the update keeps the old list until its CLI reconnects.
- Adding a tool means choosing its resource and verb, a topic in `TOPICS` and a
  section of the guide that answers it; the tests hold each tool to a topic and
  the tool list to a size.
