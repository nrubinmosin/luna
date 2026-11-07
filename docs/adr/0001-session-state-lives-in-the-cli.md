# ADR-0001: Session state lives in the CLI; the Rust core stays thin

- **Status:** Accepted
- **Date:** 2026-08-12 (the first commit, `28ab7ed`, already worked this way)

## Context

Claude Code and Codex already keep everything about a conversation: the
transcript or rollout, the session registry, titles, the model a turn ran on,
the context used, worktrees. A wrapper that mirrored that state in a database
of its own would have to keep it in step with two CLIs that change weekly, and
would be wrong whenever a session was driven outside Luna or a CLI changed its
format.

## Decision

Luna multiplexes terminals, folders and accounts and reads everything else from
the CLIs' own files.

- The Rust side knows pty sessions by id and account folders, not "chats". The
  chat list, the board layout and UI settings are persisted frontend state
  (Zustand `persist`, see [frontend](../architecture/frontend.md)).
- Status, titles, context use and the model a turn ran on are read from the
  CLI's registry, transcript or rollout (`claude/session/`, `codex/session/`).
- An account is a folder (`accounts.rs`): no database, and chats remember
  provider and account name, so moving the accounts root needs no migration.
- Worktrees are made by each CLI's own `--worktree`; Luna only finds them again
  ([worktrees](../architecture/worktrees.md)).
- Resuming a chat is the CLI's `--resume` / `resume`, after a core restart or an
  app update alike.

## Consequences

- Smaller IPC surface, and the UI evolves without touching the core.
- Status is right whether or not the chat has a pane, and for sessions that
  survived a Luna restart.
- Luna depends on undocumented on-disk formats. Every observation is recorded in
  [cli-compatibility.md](../cli-compatibility.md), and readers fail soft
  (unknown lines are skipped, not fatal).
- What the core must remember on its own (agent lineage, debts, helpers)
  became a deliberate exception with its own persistence
  (`helpers.json`, `chat-ties.json`; see
  [background helpers](../architecture/background-helpers.md)), and holds no
  conversation text.
