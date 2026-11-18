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
| `entry.rs` | what holds a worktree against its removal (`Hold`, `Place`, `Look`), a start on its way in (`entering`) and a removal's hold (`removing`) |
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
  Background helpers report theirs via `background::working_dirs`, and what
  they hold against a removal via `background::holds`.

## Removal

- Deleting a chat takes its worktree and its `worktree-*` branch by default,
  unless another session or helper works in it (`worktree_shared`, asked once
  the deleted chat's session has quit, so one that went in meanwhile counts).
  One kept is said: the chat's delete answers `worktreeKept` (`Deleted`), and
  the window's status bar says "worktree kept" until clicked away; a helper's
  delete answers `worktreeLeft` with why.
- **What holds a worktree** (`entry.rs`). A session holds the places it is
  in (`Hold`), resolved once, as it starts: its folder and the worktree it is
  started into by name (`--worktree <name>` from the project folder, in it
  from its start, not only once Claude Code's registry says so), each where
  it led then, and, for a session asking its CLI for a new worktree whose
  path the CLI picks, where that CLI makes them (`Unplaced`, below). The
  start holds the same `Hold` on its way in (`entering`), from before its
  launch until its session is in the map, which then carries it
  (`Session::hold`). A helper holds the same of its folder (and, sent into
  a new worktree, where its CLI makes them), resolved as it starts, on its
  way into the registry and then with its entry for as long as it is there
  (`Entry::hold`); one a new core takes up is resolved as that core takes it
  up. A folder the CLI says it moved to later (Claude Code's registry, Codex's
  rollout, a helper's stream) is held where it leads as it is read and, if
  it is under a place the session started in by that place's spelling, also
  where that place led as it started, the rest on top (`Hold::add_at`): a
  session or helper started through a linked project folder keeps its
  worktree from a delete by the real path, the link removed since or not,
  and a `.claude/worktrees` that is itself a junction elsewhere is followed.
  A helper's folder is held as it leads now as well.
- **Places.** Paths are compared by their spelling as `path_key_loose` has
  it (case and either separator alike on Windows, case alike on macOS,
  neither on Linux) or by where both lead, resolved through links and
  junctions (the nearest folder of it there, the rest on top, for a worktree
  not made yet) (`Place`). Only a folder that is not there counts as not
  made yet; one on the way that does not read (access denied, a link that
  loops) leaves where the path leads unknown. What may keep a worktree
  depends on the look (`Look`): a delete of a chat's or helper's own
  worktree compares such a path by its spelling alone, as none of Luna's own
  sessions has one and an unrelated one would keep every worktree from every
  delete; the sweep, which only offers, takes it for in every worktree, and
  is the only look a session making a new worktree keeps anything from (a
  delete's worktree is the deleted session's own, which no such session can
  be making: theirs is new). A start on its way in is compared with a
  removal as the sweep compares, whatever the removal (`keeps_on_its_way`):
  one with a place that does not read is refused while any worktree is being
  removed, which costs it a retry once the removal is through (`entering`),
  and one already on its way in as a removal looks, or letting go of its way
  in while it looks, has the removal keep its worktree, said as any kept one
  is. A removal whose own worktree does not read holds off every start while
  it runs. The refusal says which: the worktree the start goes into is being
  removed, or a folder on the way does not read, so the start cannot be told
  to be outside it.
- **The removal's hold** (`removing`). From the look for whoever is in the
  worktree to the end of its removal the worktree is held: a start on its
  way in that may be in it keeps it, and one that comes during the removal
  is refused, saying so. The look reads the disk (where a session works
  now), and a share or a mapped drive slow to answer would hold every start
  behind the holds' lock: it runs outside it, and a start that may be in the
  worktree and lets go of its way in while it runs (listed by then, maybe
  after the look went by) marks it, and the worktree stays (`Looking`).
  Starts elsewhere mark nothing. A helper's delete with `dropWorktree` and
  the orphan sweep hold each worktree they remove the same way.
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
- The orphan sweep scans every folder and each account's `worktrees/`, and
  only offers: the person ticks what goes and confirms. In use is every
  chat's worktree the window knows, the cwd of each entry in each account's
  Claude Code registry and the user's own (sessions started in a terminal
  too) whose process is still there (a killed one leaves its entry behind),
  and what Luna's own starts, sessions and helpers hold (`orphans.rs`
  `luna_holds`: the starts on their way in read first, then the session map
  and the helpers, so a start listed between the two reads is in one of
  them), all compared with each worktree as places: a chat parked in a
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
  which only leaves them unoffered. A helper's makers are read before its
  folders, so one that learns its path between the two reads is in one of
  them; a session's both come of one read. The sweep's last look before each
  removal, under the removal's hold, reads Luna's own and the registries
  again (`kept_from_removal`); the chats' worktrees are the window's, as it
  sent them when the sweep began.
- **A link retargeted at the start.** A start's places are resolved just
  before its CLI is spawned (`pty/spawn.rs`), and the CLI is spawned in the
  path as spelled, not in where it resolved: a junction or symlink on the way
  retargeted between the two can start the CLI in another worktree than the
  one held, until the CLI says where it works (Claude Code's registry,
  Codex's rollout), which is then held as well. Spawning in the resolved path
  would change the cwd the CLI sees, and Claude Code keys its project folders
  (transcripts, memory, settings) by that cwd, so it is not done.
- **A move through two links, the outer gone.** A folder a session says it
  moved to under a place it started in is held where that place led as it
  started with the rest of the path on top as spelled, and where the whole
  path leads as it is read (`Hold::add_at`). A rest that goes through a link
  of its own (a project reached through an alias, its `.claude/worktrees` a
  junction elsewhere) is followed by neither once the outer alias is
  removed, however early the move was read: the hold is built afresh at
  each look (`pty/probe.rs` `holds`), so a delete naming the worktree by
  its real path can miss that session and remove the worktree it is in.
- **A path that does not read, to a delete.** Only the sweep counts a
  listed session's or helper's path whose way does not read (access denied,
  a link that loops) as in every worktree. A delete of a chat's or helper's
  own worktree compares it by its spelling alone, so a session in that
  worktree by such a path alone is not seen there. A session's places are
  resolved as it starts, and one that does not read then is kept as not
  read: on its way in it keeps every worktree a delete looks at, but once
  it is listed a delete compares it by its spelling alone, as it does a
  CLI's word of a move through a folder that does not read.
- What the sweep cannot see: a Claude Code session started in a terminal
  after the last look read its registry (an outside process takes no lock
  of Luna's), a Claude Code session on a config folder that is neither one
  of Luna's accounts nor the user's own (`CLAUDE_CONFIG_DIR`, else
  `~/.claude`), and any Codex session Luna did not start, on a Luna account
  or not: Codex keeps no registry of live sessions
  ([CLI compatibility](../cli-compatibility.md#files-and-status-all-1)), only a
  rollout per thread. A worktree such a session sits in may be offered; a
  sweep is the person's to confirm, and git refuses nothing then, so it is
  removed with the files in it.

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
