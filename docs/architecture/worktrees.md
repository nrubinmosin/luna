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
| `refs.rs` | HEAD read off the files (`head_at`), the repository a folder is in (`repository_key`), and the fetch |
| `remove.rs` | removal with its branches |
| `orphans.rs` | the unclaimed and their sweep |
| `named.rs` | a worktree asked for by name (`name_ok`, `worktree_arg`) |
| `inspect.rs` | what deleting one would lose (`commits_lost`), and where a helper's work stands (`work_in`: last commit's subject, `git status --short` capped) |
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
  unless another session or helper works in it (`worktree_shared`, asked once
  the deleted chat's session has quit, so one that went in meanwhile counts).
  From that question to the end of the removal the worktree is held
  (`entry.rs` `removing`): a chat or helper start on its way in, not yet
  listed, keeps it, and one that comes during the removal is refused. A
  helper's delete with `dropWorktree` and the orphan sweep hold each worktree
  they remove the same way, the sweep asking again whether anyone is in it.
  The question reads the disk (where each session's folder leads), and a
  share or a mapped drive slow to answer would hold every start behind the
  holds' lock: it is asked before that lock, and asked again where a start
  let go of its way in meanwhile (listed by then where the question looks),
  up to 8 times, past which the worktree stays.
  A Claude Code chat started into a worktree by name (`--worktree <name>`
  from the project folder) is in it from its start (`Session::enters`), not
  only once its registry says so. The holds and the sharing check compare
  places, not spellings (`entry.rs` `Place`): a path is in a worktree by its
  spelling as `path_key_loose` has it (case and either separator alike on
  Windows, case alike on macOS, neither on Linux) or by where both lead,
  resolved through links and junctions (the nearest folder of it there, the
  rest on top, for a worktree not made yet), so a session started through a
  linked project folder keeps the worktree a delete names by its real path.
  Only a folder that is not there counts as not made yet: one on the way
  that does not read (access denied, a link that loops) leaves where the
  path leads unknown, and such a path is taken for in every worktree, as a
  miss deletes one with a session in it.
  Removal is
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
  live session or helper sits in is deleted. In use is every chat's worktree
  the window knows, every live cwd in each account's Claude Code registry and
  the user's own (sessions started in a terminal too), and Luna's live
  sessions and helpers, all compared with each worktree as places (`Place`),
  each path resolved once a look (`orphans.rs` `Keepers`): a chat parked in a
  worktree by its real path keeps it from a sweep of the project opened
  through a link, and the other way about. A worktree younger than 5 minutes
  is no orphan (`GRACE_SECS`), nor, however old, is one that may be a live
  session's whose path Luna has not learned yet (`Unplaced`): a chat or a
  helper's running turn started with a new worktree whose place its CLI picks
  (Codex's `--worktree`, a bucket named for the first 4 hex of a fresh UUID
  under `<CODEX_HOME>/worktrees/`, said only in the rollout of its first turn;
  Claude Code's `--worktree` with no name, said in its registry once it is
  up) keeps every worktree where that CLI makes them for that folder: under
  the folder's `.claude/worktrees/`, or under that account's `worktrees/`.
  Whenever it appeared, by no clock: a wall clock stepped back while the CLI
  made its checkout and put right before Luna looked dates it before the
  session's start. Once the session's folder says where it went, or it ends,
  the usual check applies; a Codex session never given a prompt holds its
  folder's worktrees in that account from the sweep for as long as it runs,
  which only leaves them unoffered. The makers are read before the folders
  in use, so one that learns its path between the two reads is in one of
  them, and the sweep's last look before each removal reads all of it again
  (`kept_from_removal`). Deletes take only the deleted chat's or helper's own
  worktree, which no such session can be making: theirs is new.

## Fetch before a Claude Code worktree

With `fetchBeforeWorktree` on, Luna fetches origin's default branch first (10 s
at most, never asking for a password); Claude Code branches the worktree off it
and refetches only once a day itself. Codex checks out the folder's current
commit, which no fetch moves, and Luna never moves the user's own checkout.

## One helper's worktree at a time per repository

Both CLIs make a worktree with `git worktree add` (Codex with `--detach`) as a
helper's first turn starts, and two at once in one repository have failed with
"unable to stat just-written file". A spawn with `worktree: true` holds its
repository's queue (`agents/spawn/worktree.rs` `worktree_queue`, keyed by
`repository_key`, the common git folder) from starting its turn until the CLI
has said where its worktree is, the turn has ended, or 20 s are up. A person's
chat started with a worktree is not queued (`pty/start.rs`): it starts when
the person asks, and a race with a spawn is left to its rare chance.

## Invariants

- Every destructive path is gated on recognizing the worktree as the CLI's.
- One gone from disk is matched by where it was: Claude Code removes a clean one
  on exit and makes it again by name.
- What git cannot tell counts as work.
