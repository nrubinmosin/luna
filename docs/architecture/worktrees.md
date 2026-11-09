# Worktrees

## Responsibility

Each CLI makes its own worktrees from `--worktree`; Luna never makes one. It
finds them again: to remove one with its chat, to report what deleting would
lose, and to sweep the ones no chat claims.

## Key modules (`worktree/`)

| Path | What it holds |
| --- | --- |
| `mod.rs` | the git command |
| `paths.rs` | which folder a worktree is of (`is_worktree_of`, `project_of`, `root_of`, `in_checkout`) |
| `refs.rs` | HEAD read off the files (`head_at`), and the fetch |
| `remove.rs` | removal with its branches |
| `orphans.rs` | the unclaimed and their sweep |
| `named.rs` | a worktree asked for by name (`name_ok`, `worktree_arg`) |
| `inspect.rs` | what deleting one would lose (`commits_lost`) |
| `src/features/chats/WorktreesDialog.tsx`, `worktrees.store.ts` | the title bar's Worktrees window |

## Recognizing a worktree

- **Claude Code:** `<folder>/.claude/worktrees/<name>`, branch `worktree-<name>`.
- **Codex:** `<CODEX_HOME>/worktrees/<4 hex>/<repo>`, detached HEAD, no branch.
  Recognized by shape plus a `.git` file (`gitdir: <repo>/.git/worktrees/<id>`)
  pointing back at the folder, and removed only inside a Codex account's
  `worktrees/` with git's own record of it.
- `project_of` goes by the `.git` file for both; the path's shape says only where
  to look. A folder under a worktree belongs to it only if no repository of its
  own lies on the way down and git does not ignore it (`in_checkout`), so a test
  project in a worktree's `test-build` is not a worktree of the main checkout.
- Worktree records come from `git worktree list -z`, so a path with a newline
  forges none. A folder spelled in 8.3 short names matches git's long names.
- Where a session actually works is `pty::ActivityProbe::working_dir()` (Claude
  registry / Codex rollout); the root is found from anywhere inside
  (`worktree::root_of`), as Claude Code moves its cwd with the shell's `cd`.
  Background helpers report theirs via `background::working_dirs`.

## Removal

- Deleting a chat takes its worktree and its `worktree-*` branch by default,
  unless another session or helper works in it (`worktree_shared`). Removal is
  `git worktree remove -f -f`, since Claude Code locks its worktree and a killed
  one does not release the lock. Only `worktree-*` branches are ever deleted.
  A Codex bucket dir goes once empty.
- **`luna_session_delete` with `dropWorktree`** (`worktree_to_drop`: not one handed to
  the session or shared) runs `git status --porcelain` (submodules not hidden)
  and counts commits that would be lost (`commits_lost`): those HEAD or a going
  `worktree-*` branch reach and no staying branch or remote does. A branch the
  session named itself stays. Ignored files do not block (build output would
  block every delete) and are said to go. It refuses with a short list unless
  `force`. Clean, the session is stopped and its leftovers ended
  (`stop_for_good`), then it looks again; anything new keeps the worktree
  (`worktreeLeft`).
- The session goes before its worktree, since its process holds the files: a
  removal that still fails is `worktreeLeft` + `worktreeError` in the result,
  not an error. The log names branches deleted and kept.
- The orphan sweep scans every folder and each account's `worktrees/`; nothing a
  live session or helper sits in is deleted.

## Fetch before a Claude Code worktree

With `fetchBeforeWorktree` on, Luna fetches origin's default branch first (10 s
at most, never asking for a password); Claude Code branches the worktree off it
and refetches only once a day itself. Codex checks out the folder's current
commit, which no fetch moves, and Luna never moves the user's own checkout.

## Invariants

- Every destructive path is gated on recognizing the worktree as the CLI's.
- One gone from disk is matched by where it was: Claude Code removes a clean one
  on exit and makes it again by name.
- What git cannot tell counts as work.
