# ADR-0016: Luna is a runtime, not a provider configurator

- **Status:** Accepted
- **Date:** 2026-11-08

## Context

Luna starts Claude Code and Codex with arguments, environment variables and
files of its own, and writes into account folders. Over time it came to add
more than running a session needs. An inventory of every item (one per flag,
variable, file or write) found twelve that changed what a CLI does without the
person having asked:

- the guard hook in every chat with Luna's tools;
- Codex's `approvals_reviewer="user"` over the account's own reviewer;
- `windows.sandbox="unelevated"` over one the user had set;
- `check_for_update_on_startup = false` written into a new account's
  `config.toml`;
- a folder's trust written into the account for every new chat, an agent's
  included;
- `--skip-git-repo-check` for every helper;
- `CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD`, which has Claude Code read
  the `CLAUDE.md` of every folder added to a session, not only the library's;
- and workarounds and writes no one could see (the Windows sandbox's PATH and
  git TLS backend, the helpers' per-run trust, the library's layout).

None of it was shown anywhere. A person could not tell what was the CLI's own
behavior and what was Luna's.

## Decision

Luna is a runtime. It adds to a CLI what running it as a Luna session needs
and what protects Luna's own boundary. Anything else that changes the
provider's behavior happens only on the person's say, and everything Luna
applies is shown.

Every item is in one of four classes (`applied.rs` `Class`):

- **Runtime**: what Luna needs to run the CLI as one of its sessions (the
  account folder, the session's id, resume, activity hooks, Luna's own
  updates, a helper's identity, headless mode and per-run trust).
- **Boundary**: what keeps a session out of Luna's secrets (the core-folder
  read deny, ADR-0014).
- **UserAction**: what the person chose: in the new-chat dialog, in Settings,
  in the library, or through an agent they gave tools.
- **Workaround**: what the CLI cannot do on this machine without it, such as
  Codex's Windows sandbox, its PATH and git's TLS backend.

The owner's ten decisions on the deviations (2026-11-08):

1. Guard hook: chats get it only when the person ticks "Wait guard" in the
   new-chat dialog (off by default). Helpers keep it.
2. Codex `approvals_reviewer="user"`: helpers only. A chat keeps whatever
   reviewer its account or project names.
3. `windows.sandbox="unelevated"`: only when the account's `config.toml`
   names no `windows.sandbox`. A project's `.codex/config.toml` is the
   repository's say, not the user's, and does not count.
4. Windows PATH without the Store's aliases, and git's OpenSSL backend: kept
   as Workarounds, and shown.
5. `check_for_update_on_startup=false`: passed as `-c` on every Codex start,
   never written. Account creation writes nothing. Lines already in a
   `config.toml` are left alone.
6. Folder trust: the new-chat dialog asks "Trust this folder in <account>",
   unticked. Unticked, nothing is written and the CLI asks. A chat an agent
   starts never writes trust. A Codex helper is trusted for its own runs
   (`-c`), and nothing is persisted.
7. A helper's Codex trust override stays, as Runtime, and is shown.
8. `--skip-git-repo-check`: only when the helper's folder is not in a Git
   repository, and shown.
9. `CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD`: Claude Code 2.1.318 reads
   it as a switch (every added folder's memory or none). Luna sets it only
   when the library gives instructions, and narrows it with `claudeMdExcludes`
   patterns for the memory files of every other added folder: Luna's notes
   folder, the library's skills folders, and the folders in the user's own
   `permissions.additionalDirectories`. A folder above or below the one the
   session works in is left alone, since Claude Code reads those anyway
   (`claude/launch.rs` `memory_excludes`).
10. Library layout: kept as a UserAction, and shown.

Mechanics:

- The pure launch builders (`claude::launch::chat_args`,
  `codex::launch::chat_args`, `background::command::command_for`) return the
  items they applied beside the arguments and environment, as
  `Vec<Applied { key, class, value, why }>`. A value names a token by the
  variable it travels in and a hub URL by what it is for, so neither appears.
  A file Luna writes is named by its path, not its content.
- `Launch` carries them. The registry keeps a session's latest
  (`agents::set_applied`) when its process starts. A helper's are replaced
  every turn. The core answers `applied_by_luna(chat_id)`.
- Writes into an account outside a launch go in a small ledger,
  `<data>/applied-ledger.json` (`applied::record`, under the account
  settings' lock). For now that is folder trust. The library's layout is read
  from the library's own ledger.
- The window shows them in each chat's "What Luna applied" (the ⓘ button on
  its title bar), grouped by class, each with its why.

## Consequences

- New code that adds anything to a CLI's start declares it as an
  `applied::Item`. The `items!` macro will not take an item without a class
  and a why. The test `every_item_has_a_class_a_why_and_a_key_of_its_own`
  checks that each key is unique and each why is a sentence. The launch golden
  tests check what each kind of start applies, and that no value carries the
  session's token or a hub URL.
- Chats behave more like the bare CLI. A chat no longer refuses a command that
  waits forever unless it was asked to. A new folder shows the CLI's own trust
  prompt unless the person ticks the box. Claude Code cannot show that prompt
  for a worktree it makes, so the dialog will not create such a chat until the
  box is ticked or the worktree is turned off. Quick chat opens the dialog for
  an untrusted folder.
- Existing chats with Luna's tools lose the guard they had by default.
- A visible Claude Code helper in a folder its account has not trusted shows
  Claude Code's trust prompt. Claude Code has no per-run trust that changes
  nothing else (`CLAUDE_CODE_SANDBOXED` also lifts other checks), so Luna
  does not use one. Visible helpers are on their way out (0.11.0).
- Folders added mid-session with `/add-dir` are not covered by the narrowing
  in decision 9. Only folders known at the start are.
