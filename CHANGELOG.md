# Changelog

All notable changes to Luna are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/) (pre-1.0: a minor version may change
behavior). Releases before this file existed are summarized per minor version
from the git tags; `git log v<a>..v<b>` has the detail.

## [Unreleased]

### Added

- A native window on GPUI, Luna's only window: its own title bar (Luna's
  name, the theme button, minimize, maximize and close), the core banner,
  the sidebar and the board, in one font (Cascadia Mono, carried in the exe)
  and a retro, high-contrast, static look in light and dark, or as the
  system is. It finds or starts the core, and while a core is being found or
  replaced it says it is waiting for it, never that the core is not
  running. The window is a crate of its own (`src-tauri/crates/window`) over
  a host in the app, on GPUI's Windows crate with Luna's three patches
  (`src-tauri/vendor/`). Where it cannot open (no usable GPU device), Luna
  logs why and, on Windows, says so in a message box naming the log.
- The native window's library, settings and accounts windows, in-app over
  the main one ([native window](docs/architecture/native-window.md#panels)):
  the library's entries, whom each is meant for and whose sessions it is
  for, folders and Luna's system texts, with gpui-component's editor
  (Markdown highlighted, a steady caret) for their files; Settings' sound,
  accounts folder, new chats, helpers, trusted folders, the CLIs' version,
  Check and Roll back, and Luna's update, whose Update... opens the status
  bar's confirmation, so it freezes, stops and saves as the chip's install;
  the download and the install name the offer confirmed, and the host
  refuses them once a check has put up another;
  the accounts with their limits, added, renamed, deleted, and the parent
  whose settings the others copy. A skill can now be renamed
  (`library_rename_skill`): without following a link, replacing a folder of
  the new name or leaving its `name` half rewritten, and saying whether it
  was done, undone, or left partly done. The editor's save is bound to the
  library and the file as they were read: the library's folder by its
  identity on the drive, read and written through a handle held to it, so a link
  there retargeted or a folder made again gets nothing; a file changed on
  disk since is not overwritten unasked (Reload, Overwrite), and text not
  saved when the window goes (the tray's Quit or Restart window) is offered
  at the next open, read back only as a private file of Luna's; throwing it
  away leaves text typed since. A save whose layout after fails says it was
  saved. Every
  way the editor's text is replaced asks first. Sync is the core's pass
  (Luna's own copies written over or put back, unwanted ones taken out, a
  changed one backed up first; files Luna did not write left alone), and
  says how many copies it wrote and took out, how many places it left
  alone, and each it could not write or take out, with why; the pass's
  answer now lists what it wrote, took out and could not take out. Deleting an account is bound to the
  folder its question named, removed through a handle held to it without
  following a link or crossing into a mount (on Unix, one on another
  device), and names the chats still set to it. Rename takes the chats on
  the account along. Sign in on a signed-out account runs the CLI's login
  (`claude`, or `codex login`) in a terminal over the accounts window, which
  closes with the CLI and ends its session. The title bar's Menu opens
  the library and the accounts, the status bar's Settings button the settings
  (and its Background count the helpers' popover), and what Settings keeps for new chats reaches the
  window's workspace. The test
  build's `LUNA_TEST_DRIVE` file drives them in a smoke run without global
  input.
- The native window's dialogs ([native window](docs/architecture/native-window.md#dialogs)):
  **New chat** (Ctrl+N and the button: folder with a picker, account, model,
  effort, the CLI's modes, Luna's tools, trust, and a worktree box only for a
  folder in git, all opening on what the CLI's own settings and Settings'
  new-chat defaults say), **quick chat** (Ctrl+Shift+N, the dialog's own path
  in one step, handing over to the dialog for trust or a failure), **delete
  chat** (asks first, with "stop or keep its helpers", each box a yes of its
  own; a worktree the CLI made stays on disk), the **helpers popover** (a chat's helpers: watch one
  as plain text, open it as a chat, stop it, delete it) and a **crash
  screen** at start when the run before ended without closing itself, with
  the panic or exit code, where the log is and Restart window (the panic hook
  and the exit watcher now keep what they saw in the run's record). The wait
  guard, Codex's hooks box, keeping a start for a folder, auto-handoff,
  removing a worktree and moving a helper to another chat are cut with the
  rest of ADR-0030's list: the window's chats keep no guard, a start sends
  no guard, handoff or hooks argument, and the new-chat dialog offers the
  worktree box from the folder check's `inGit`.
- `scripts/gpui-shots.ps1`: the native window's demo scenes as PNGs, shot
  off screen inside the app; `.github/workflows/gpui.yml` builds and tests
  the window on macOS. Its `-Fit` runs the window crate's fit test, which
  CI runs with the crate's other tests: every scene and popup at the window's
  own sizes and the small laptop screens the WebView window was measured on,
  failing on a control off the window, squeezed to nothing, cut off inside
  a dialog or panel that is itself on screen with no scrolling to it, or
  covered where a click would land, and on a drop-down left open once its
  face scrolls away.
- The native window's terminal: one chat's CLI, shown and typed into, on
  alacritty_terminal's grid and parser, parsing on a thread of its own and
  drawing only when there is something new (no blink, no timer). Full key
  encoding with modifiers and the cursor-key mode, bracketed paste, the
  mouse in SGR and X10, selection by drag, word and line, copy and paste
  (Ctrl+C copies a selection and interrupts without one), files pasted or
  dropped typed as quoted paths, a picture on the clipboard pressing the
  CLI's own image-paste key, the scrollback, the IME's composition at the
  cursor, box drawing as shapes, truecolor and every text attribute, in the
  window's light and dark looks. A chat reattached replays the core's
  backlog as the WebView window does, and takes keys once the CLI's modes
  are back; its exit and refused keys are said in the terminal, and it waits
  for a core being found or replaced and starts its chat over in the next
  one, or after the core's start-over (a shutdown of the PC called off).
  A reattach to a core that numbers its output takes what came after the
  backlog exactly, nothing twice and nothing missed, and says it is
  reattaching; one that misses output, fails or takes past 15 s starts
  over rather than take keys without the CLI's modes. Pasted text loses its
  control characters (but tabs and line breaks), DEL among them, with
  bracketed paste off as well as on, and keys typed while a paste reads the
  clipboard go after it, encoded by the modes they go with; a clipboard
  that does not answer in 5 s loses its paste, said in the terminal, and
  what waits behind pastes is bounded. Output it cannot keep up with is
  dropped and said, never held without bound, and a synchronized update left open is drawn at
  its deadline. The test build's driver and stand-in clipboard are compiled
  into the test build alone (the `luna_test` cfg).
- The native window's chats and boards: the sidebar of the board's chats by
  folder (fold a folder, rename, delete, status, draft and pane number per
  row; pinned or collapsed to a strip), five fixed boards I-V with layouts 1
  to 4, Clear board and Auto-tile on the title bar, each pane's title bar
  (status, name, model and effort, account, context, the session id to copy)
  and menu (rename, copy the id, move to or swap with another pane, take
  off the board), the peek (a chat not on the board full size over it, To
  board; Esc closes it until its terminal is clicked or typed in, then Esc
  is the CLI's and Shift+Esc closes) and the window's keys (Ctrl+N,
  Ctrl+Shift+N, Ctrl+Shift+1-4, Ctrl+1-4, Ctrl+Shift+W, Ctrl+Tab, Esc,
  Shift+Esc), the digits by the physical key on any keyboard layout. Chats
  keep their status, titles, context and drafts from `sessions_meta` every
  4 s (an answer from before an exit or a core restart, or a call that
  failed or hung, changes nothing; rows say "no core" while there is none),
  rest when their own session exits, notify once per wait, and resume the
  session they have now when the core is replaced (only the chats shown
  start again). Nothing is dragged. The window keeps its chat list in
  `window/chats.json` in the data folder, a file nothing else writes (the
  window's place is in `window-state.json`, where a file that is no place
  is put aside, not written over), written synced as the page's storage was
  (at once after a quiet spell, then at most every 500 ms; a chat added or
  deleted, its session, account or board at once; on quit), each write
  confirmed and a failed one retried, with a wait that a burst of changes
  does not reset, and said. Quit waits for the list to be written and,
  where it is not, keeps the window up with Retry and Quit anyway (a
  sign-out or shutdown tries once more and logs). A list that cannot be
  read is never written over (Retry reads it again, one read at a time, and
  an answer that comes after a change is let go of); one that reads only in
  part, a value this build does not know or a chat listed twice included,
  is used for what reads and kept aside as it was; one that is no list is
  put aside; a line says which. On its first start the window imports
  0.19's chat list from a private copy of the WebView's Local Storage: the
  data folder's `webview` store; only where that is not there at all,
  `%LOCALAPPDATA%\rm.luna`, for any copy as 0.19 took it (the test build
  its own), then, only for the installed Luna (that store is per user),
  `ai.sprouter.luna`, for a Luna last started on 0.6.31 or before. A
  store that cannot be looked at or read is said, never passed over for an
  older one; one reached through a link or junction is refused; the one
  taken is logged by path (rusty-leveldb, the origin 0.19 wrote to over a
  fuller list of another, even where its value does not decode, a copy taken again if the store changed under it
  or could not be listed or copied), saying so in a line with Retry when it
  cannot and trying again on later starts while the list stays empty. A
  list from before 0.3.0 (an older format) is not imported as an empty one:
  a line says so, without Retry. Chats and folders the import leaves out
  are counted and the old list kept aside as it was, as for the window's
  own file. Quit writes the
  list before it goes, and again if it changed while the host wrote;
  the data folder is synced once per run before the first write is
  acknowledged; an update's install writes the list, theme and place before
  its setup starts, or is refused (`update_install`'s `unsaved:` error, with
  `anyway` to go on). Ctrl+1-4 go by
  the key pressed, never by a digit merely held (on Canadian French, Shift+3
  and the keypad's Divide, both `/`, are told apart by Shift), the looks at the
  sessions give their request up with the window, and the accounts are
  asked for again until they come. The test build's `--dev-open` terminals are gone: the
  board opens chats.
- `resize_session` takes the terminal's size in pixels too (`pixelWidth`,
  `pixelHeight`), which the native window sends and the core passes to the
  PTY.
- `write_session` takes `reply: true` for a terminal's own answers to the
  program's queries (sizes, colors, device attributes): the core writes them
  straight to the PTY and does not count them as a person typing or as a
  draft. The native window's terminal sends its replies so; the page sends
  none. Only a write that is one such answer whole is taken for one;
  anything else so marked goes as keys, held and counted as they are.
- Each `pty://output` chunk carries `offset`, where its text starts in all
  the session has sent, and a start that reattaches answers with
  `backlogEnd`, the offset its backlog ends at, and `modes`, the session's
  modes as of there (UTF-8 mouse, 1005, now kept among them): the native
  window's terminal takes the output after the backlog exactly. The page
  ignores them.

- The desktop services the native window of 0.20 uses in place of
  Tauri's plugins (`src-tauri/src/desktop/`,
  [desktop services](docs/architecture/desktop-services.md)): the tray, toasts
  and macOS notifications that open their chat when clicked, one window per
  data folder with a second launch bringing the first up, folder and file
  pickers, a clipboard read that tells files, text and images apart, the
  window's size and place kept in the data folder, and an updater that reads
  the same `latest.json`, checks the same signature and runs the installer as
  0.19's does, so 0.19 updates onto 0.20 and 0.20 onto later releases. The
  updater takes only https and only an NSIS setup, stages it privately under
  a name of its own in a data folder that is no link (a OneDrive folder's
  included) and on a local fixed drive, holds that folder from its last
  check through the write, keeps the setup open from its last check until it
  has started, starts it by the path that handle resolves to, and passes
  Luna's arguments on to the restart only whole, with a unit of the setup's
  command line to spare. A pasted PNG is taken only once all of it decodes,
  and a TIFF that gives its size twice is not taken.
- An NSIS installer built without Tauri's bundler
  (`./scripts/build-windows.ps1 -Installer`), adapted from Tauri's
  template (credited, its license among the third-party notices), installing
  as today's in place over it, without the WebView2 bootstrap, and starting
  Luna again after an update through the desktop's shell, with the arguments
  it ran with as they stood, only where it would not run elevated (and not
  at all where that cannot be told); the build holds the exe it installs to
  x64; and `scripts/macos-app.sh`, which
  assembles an unsigned `Luna.app` for a macOS runner.

- The native window's desktop services, wired in: Luna's tray (Open,
  Restart window, Quit; a click opens the window), closing to the tray with
  the sessions running, the window opening where it was left (on the display
  it was on, maximized if it was, minimized or not), and the end of the
  Windows session logged with its reason and then quit as the tray's Quit
  does, the window's place and theme written and the tray icon removed. Its
  host takes notifications (a click opens the chat), the clipboard, folder
  and file pickers and the updater's check, download and install, for the
  surfaces of the window that come next; the tray's awake dot is the
  native window's status bar's, which works out who holds the PC as the page's
  does. An update installs only the release on offer when its download
  finished, one at a time, and never while the tray's Quit or Restart window
  is under way, nor they while it is; a check during an install cannot
  change what it installs, and an install that fails never takes back a
  Quit's reason for leaving. A tray Restart whose old window outlives it says
  so in the log instead of vanishing. The installer refuses, in update mode,
  to put an older Luna over a newer installed one (an older copy run
  elsewhere could be offered one), by full version order (rc.1 never over
  rc.2, a release candidate never over its release) read from the install
  folder's own exe, one setup at a time, and lets a same-version repair of a
  folder its record names go ahead; the updater writes the setup through
  handles on the data folder it checked, so a link above that folder changed
  meanwhile leads the write nowhere else. The window comes back on the
  monitor it was on, wherever that monitor has moved.
- The native window's status bar and orphan line. The bar counts the chats
  working, waiting (a click goes to the next one, as Ctrl+Shift+W does) and
  in the background; shows every account with a meter per usage window and
  how long until it resets, on the page's own polling (never faster), giving
  detail up a step at a time as the window narrows; lights a chip for a new
  Luna release (click for the notes, then download and install; the chat
  list takes no change from its last write until the installer has it, and
  the installer starts only once that write is confirmed; an install that
  cannot first write what Luna holds is refused and offers Retry, Install
  anyway or Cancel, and Cancel starts again the sessions stopped for it) and
  for a CLI downloading or failing to update; and carries the power chip:
  keep awake on or off, "shut down when quiet" (or restart, hibernate,
  sleep) with its countdown and Cancel, a Cancel that fails said on the chip
  with Retry, "power: no core" and no countdown while the core is out of
  reach, who holds the PC awake, and the tray's awake dot. A Settings button
  is at its right. The orphan line, "N orphan sessions: kill all", shows
  only while a session runs with no chat, and asks before it kills: it kills
  only the sessions it asked about, looked at again first (alone: a look
  under way is let go of), and asks anew if they changed or the question was
  ended meanwhile. Nothing in them moves: the bar redraws only when what it
  shows changes, and the one clock is the countdown's, a second at a time
  while it runs. The core numbers its power states (`revision`), and the
  window stamps each with the core it came from, so it drops one older than
  the state it shows or one of a core since replaced. The core is sent the
  accounts' usage for `luna_account_list`, as the WebView window sends it.
- **Roll back** for each CLI in Settings → CLIs of the new window, beside
  **Check**: it goes back to the version Luna keeps before the current one, by the same checks as
  the automatic rollback (a whole install, not below the oldest this Luna
  runs, every file matching its checksum), and says why where it cannot. The
  version left is skipped while it is the newest release, until a different
  one comes out. The button names the version it goes to and is shut, with
  the reason beside it, where it would be refused; it asks once ("Roll back
  Claude Code from A to B?") and then says what it did (A → B) or why not,
  looking again at the versions when the current one had moved.
- A background helper's turn that fails is recorded in Luna's log with its
  cause: how its CLI went, the error its stream gave and the end of its
  stderr, so that a "failed" told to its caller has a reason on record.

### Changed

- The repository's US English check and its tests run on PowerShell, so CI
  can check tracked files without Node.

- `LUNA_DEV_TOKEN` turns off the single-instance claim and the run record
  only in the test build: a release ignores it, so a variable left in the
  environment can no longer open a second window on the same chat list or
  hide a crash from the next start. (The scripted MCP caller it registers is
  unchanged.)
- A provider's accounts folder that is there and cannot be read (denied, a
  share that dropped) now makes `list_accounts` fail, where it listed no
  accounts: the page and the native window ask again instead of keeping an
  empty list, and the callers that listed without a folder now see its error.
  A folder that is not there is still no accounts.
- One Luna window per data folder: a second launch brings up the window
  already running and exits. The single instance is Luna's own, in place of
  tauri-plugin-single-instance. The folder is told by where it is and, where its drive keeps file ids for
  good, by what it is, not by how its path is spelled (a long and a short
  name, a junction); a data folder made again under a running Luna is still
  its, a FAT drive or a share cannot stop the start, and a launch whose claim
  fails stops, said in the log, rather than open a second window.
- The third-party notices cover the native window's crates (GPUI, its
  components and what they bring); the license texts their packages leave
  out are kept in `third-party-licenses/`, and the vendored GPUI platform
  crate's own are read from `src-tauri/vendor/`.
- Codex helpers and leads run on `codex app-server` instead of `codex exec`
  (ADR-0032): each turn is one server, its thread started or resumed with
  the helper's folder, model and role, and the turn started in it. Text
  sent while a turn runs now steers that turn instead of waiting for the
  next. `luna_session_stop` and a send with `interrupt` ask the turn to stop
  and kill it only if it has not within 5 seconds. A Codex helper's approvals
  come to the session that spawned it as asks, answered with
  `luna_session_answer`, as a Claude Code helper's do: `approval` takes
  `never`, `on-request` or `auto-review`, and left out, the account's config
  decides (Luna no longer forces `never` nor names a reviewer). A usage
  limit ends the turn as `usageLimit`. The oldest Codex Luna runs is now
  0.169.0. Text a Codex helper was sent just before a restart is never sent
  again by Luna: found in its thread it counts as delivered, else it goes
  back to its sender; a thread interrupted before its first turn is started
  afresh, and a Codex server that never starts its turn is ended as failed
  instead of hanging; a started turn is never ended by the clock, and its
  parent hears when it has gone quiet, as for any helper.
- A Claude Code session's folder skills (the library's skills a folder
  gives to the sessions started in it) are a plugin Luna makes for that
  session alone, passed with `--plugin-dir`, in place of copies kept under
  `library-folders/` and added with `--add-dir`. The plugin lies in Luna's data
  folder (`session-skills/<session>/`), private, made afresh at every start and
  every helper turn and taken away with the session; nothing is written into the
  account or the project. The plugin is named `luna-<folder>`, so its skills are
  listed as `luna-<folder>:<name>` (a skill saved as `/luna-<name>` is
  `/luna-<folder>:<name>` now): none is renamed for a name the repository or the
  account has, no plugin of the account's named for the folder is shadowed, the
  CLI gets no read access to a skills folder it did not ask for, and the memory
  files of such a folder are no longer left out in the session's settings. Its
  folders are made and taken away without following a link, only `session-skills`
  is given Luna's private access (never the data folder itself), the plugins of
  sessions that are gone are swept, and a helper whose plugin would not write
  tells its parent. The 0.19 copies of a folder stay while a session launched on
  them runs. A skill not meant for the session's role is left out of
  the plugin rather than hidden. Settings: `"claudeFolderCopies": true` under
  `library` keeps the earlier way while the plugin is piloted; the pass then
  lays out the copies again, and otherwise takes out the ones it left.
  A plugin that will not write is said as `library.skills-off` and the session
  starts without its folder skills.
- Codex hides a library skill from a session (a skill for other roles, one a
  folder keeps from an account) by the path of its `SKILL.md`, not by name.
  A skill whose front matter names it otherwise than its folder is now hidden
  too, and a repository's skill of the same name is no longer hidden with it.
  The list is held to what a Windows command line has room for (about thirty
  skills beside the role layers): the rest stay visible to the session, logged
  and said as `library.hidden-trimmed`.
- Stopping a background Claude Code helper's turn (`luna_session_stop`, Stop,
  or a send with `interrupt`) first asks its CLI to stop the turn itself, as
  Esc does: its tools stop and its conversation stays whole for the next turn.
  Only a CLI that has not answered within 5 s, or not gone 5 s after that, is
  killed, as every stop was before, and what its tools left running still
  goes with the turn. The turn ends as stopped, not failed: the lifecycle
  record's outcome is `stopped`, `luna_session_wait` gives the reason as
  `stopped`, and the notice says the turn did not finish, never "stopped on an
  error". A Codex helper's turn is asked by `turn/interrupt` the same way.
- A background Claude Code helper's permission prompts and questions come
  over its own stdin and stdout (`--permission-prompt-tool stdio`) instead of
  Luna's `luna_permission` tool, which is gone; a helper without Luna's tools
  no longer gets Luna's MCP server or a token at all. Its agent answers them
  as before. To let the questions of its background subagents through, a
  helper's stdin now stays open past its answer while a background subagent
  or workflow runs, and through the turn Claude Code starts on its end once
  that turn has begun; a background shell or monitor alone does not keep it.
  At most 8 asks are held per turn, the rest refused. A failed turn's log line has what looks like a
  token, key or URL path taken out.
- A chat started without the dialog (Ctrl+Shift+N) in a folder outside git
  starts without the worktree its kept start asks for, where it was refused
  and the dialog opened. Chats, helpers and leads are now tested in a folder
  without git on both CLIs (live suite, unit tests of Codex's trust there).
- A Codex chat's background shell no longer stops holding the PC awake after
  10 minutes. Where the rollout records no end for it (Codex 0.169's legacy
  rollouts), the claim stands until the Codex process exits, then the chat
  reads `unknown` until the next read settles it; no claim is ended by the
  clock.
- A chat's places count only the helpers at work: a turn running or waiting
  on a question, and spawns under way. A helper between turns no longer
  takes one, even with a background command it left running, so a chat may
  keep more helpers than it has places. **Helpers per chat** is 16 by
  default (was 8) and goes up to 64 (was 32). Every helper turn is checked
  as it starts: text for an idle helper when every place is taken is no
  longer refused but queued (`waitsForPlace`), and its turn starts as soon as
  a place frees, in the order such turns came; a turn a restart cut short
  goes on the same way. A spawn past the limit is still refused, naming the
  caller's own helpers at work, to wait for or stop, rather than ones to
  delete. Luna's notices to a lead start its turn even on a full line.
  `places` in `luna_session_spawn` and `luna_session_list` counts each
  session once and adds `waiting`.
- Codex role layers too long for a command line are no longer refused or
  dropped. A Codex session's role text, the library's instructions for its role
  and its constraints go in `-c developer_instructions` as before; over what a
  Windows command line holds (24,576 characters, Luna's limit, not Codex's)
  they are written whole to a file in Luna's data folder
  (`role-layers/`), and the value tells the session to read it first and again
  after a compaction. A lead's or helper's turn is refused, and a chat starts
  without its layers, only where the account's own `developer_instructions`
  leave no room even for that pointer. The Library window flags an entry only
  then.
- A Codex account's model list is its own: the new-chat dialog and a helper's
  model and effort are checked against that account's `models_cache.json` (or
  what `codex debug models` says of it), never against another account's. An
  account that has not run yet has no list to check against, and Codex refuses
  what it does not take.
- `luna_report_get` and `luna_message_list` read a Codex turn's final answer
  from its end (`last_agent_message`) when its messages carry no `phase`, and a
  turn whose end says something its messages do not is given that as its
  message; a Codex report is no longer a guess in those turns.
- On Windows a Claude Code or Codex download is refused unless every program
  file in it (`.exe`, `.dll`, or anything opening with `MZ`) carries its
  publisher's Authenticode signature ("Anthropic, PBC", "OpenAI OpCo, LLC";
  a bundled runtime DLL may be Microsoft's), checked before the version is made
  current. Codex's bundled `rg.exe` ships unsigned and is the one file excused;
  it is held only to the release's checksum. Nothing is checked off Windows.
- A Codex thread is resumed by its exact id in its own account only (its
  rollout is looked up per account and thread); the id must be a UUID (any
  form, passed on as the plain one), so a thread's name or `--last` is never
  taken for one. A notice for a Codex chat holding a draft still waits until
  the draft is sent or cleared, and now also when the draft was recalled from
  history (Up) with no key typed.
- The spilled Codex role layers (`role-layers/` in the data folder) are
  owner-only like Luna's other files, a link in their place is refused, and a
  file a running session was launched with is no longer swept after 30 days.
- A Codex helper's effort with no model list of the account's own goes as
  asked (any case), held only to Codex's built-in levels when those were read
  this run; the built-in model list is no longer kept as an account's own.
- A Codex final answer written in several blocks is found by its words, not its
  newlines, and a read that begins at a turn's end line still gives its answer.

- The library no longer judges what a skill's `SKILL.md` says. One with an
  over-long description, no front matter, or a `name` other than its folder's
  is laid out like any other, into Claude Code's and Codex's accounts alike,
  and the CLI decides what to make of it; the editor no longer checks a skill
  as you type. Luna asks only that the skill's folder reads, holds a `SKILL.md`
  and is named what a folder can be called (a skill named with capitals or
  spaces is laid out too). The description under a skill's name in the window
  is still read from its front matter, but empty is fine.

- Each CLI is kept at its newest release, and rolls back on its own: when
  two of a new version's first three sessions fail soon after starting, Luna
  goes back to the version before (once its files match their checksums),
  says so in a desktop notice and the log, and skips the failed version while
  it is the newest release. A session that ended on a sign-in, a usage limit,
  or the network or the provider's service failing (by the error the CLI ended
  on) does not count, nor one that never started. A rollback Luna was closed
  before it made is made at its next start. A
  version whose answers fail the offline check is skipped the same way; one
  the check could not read to its end (a scanner holding the fresh download)
  is asked again at the next check. Each Luna release names the oldest
  version of each CLI it runs (Claude Code 2.1.325, Codex 0.167.0), compiled
  in; nothing on the machine raises it any more, and Luna never steps down
  from the version it runs ([ADR-0030](docs/adr/0030-scope-of-0-20.md)).
- What Luna keeps of each CLI's versions is one file, `state.json` at its
  root, every change of it written whole under a lock, so the window and the
  core changing it at once never leave it half made, and a crash leaves the
  old record or the new (on Windows a power cut may undo the latest change).
  A record lost or damaged is rebuilt from the installed versions whose files
  still match their checksums; the versions it had turned away are lost with
  it. 0.19's files (`current`, `previous`, `recorded`, `bad.json`,
  `canary.json` and their locks) are read in at the first check and removed.
- The library is the source of truth: a copy Luna laid into an account (or
  keeps for a folder's sessions) that was changed or deleted there has the
  library's written over it or put back the next time the library is laid
  out, which is before every session start, and one no longer meant for its
  place is taken out, changed there or not, on Claude Code and Codex alike.
  So an edit made in an account to what Luna manages there (Claude Code's
  `/memory` on the account's `CLAUDE.md`, skill-creator improving a skill in
  place) is undone at the next session start: edit the library instead. What
  a change there had made goes to Luna's backups for 30 days first, and the
  log names each place and its backup. A file Luna did not write is still
  left alone, and so is whatever a person or a tool adds inside a skill's
  folder beside Luna's files (`__pycache__/`, `node_modules/`, a link); a
  copy that changes while the pass is at it, or that Windows will not let it
  replace, is put back as it was and waits for the next, and a start's pass
  says in the log what it could not lay out. On the first 0.20 pass,
  copies that waited under 0.19's Needs a look as changed there are written
  over (their change in the backups) and those deleted there come back.
  Copies 0.19 left in place as no longer wanted but changed there are yours
  from now on: they stay, are never taken back, and the log names them once
  ([ADR-0030](docs/adr/0030-scope-of-0-20.md)).
- The library's backups are kept only in Luna's own `library-backups`
  folder: none is written through a link or junction there, and the 30-day
  sweep takes out only Luna's own backups, never through a link. A Codex
  plugin manifest in Luna's marketplace folder is taken out only where Luna
  wrote it, and never through a link below the account's folder. Deleting a
  skill from the library is refused where its folder or the library's
  `skills` is a link, and its backup keeps every file.
- What a CLI needs inside Luna is passed at every start and never written into
  an account. Claude Code chats and logins get `tui: "fullscreen"` in the
  `--settings` Luna passes and `CLAUDE_CODE_NO_FLICKER=1`, which holds the
  renderer even after the CLI gave up on it once; Codex chats get
  `-c tui.notifications=false` and `-c tui.copy_on_select="never"` beside the
  update check they already had. The fullscreen variable is taken out of the
  environment of every CLI process Luna starts and given back to Claude Code
  chats and logins alone, so a core started from a fullscreen chat's shell
  does not hand it to helpers. Claude Code's copy-on-select is **not** set by
  Luna: Claude Code 2.1.325 reads it from the global config (`.claude.json`)
  alone, which Luna must not write and no start can reach, so it stays as the
  account has it (on unless you turned it off in the CLI). What 0.19's recommendations wrote into accounts stays there
  ([ADR-0030](docs/adr/0030-scope-of-0-20.md)).

- Development: the gate a change passes is CI on GitHub, which runs on every
  branch the operator's mirror pushes (`slice/*`, `gpui/*`, `core/*`,
  `iteration/*`); the local scripts are for the test build, the live tests and
  narrow checks, since Docker builds on one PC share a lock. New
  `scripts/ci-wait.ps1` waits for the CI run of a commit and prints how it
  ended, with the failed jobs, their steps and the tail of their logs (exit
  0 passed, 1 failed, 2 timed out, 3 no `gh`); its tests are a step of the
  gate.
- Each session, chat or helper, of either CLI, has one lifecycle record made
  of what its CLI says: a turn running, waiting for an answer, ended (and how:
  ok, failed, usage limit), what runs beside its turns (shells, crons,
  subagents), or not known yet, which counts as active. Claude Code's comes
  from its hooks, `Stop`'s `background_tasks` and `session_crons` among them,
  and its `sessions/<pid>.json` (status, `shell`); a Codex chat's from Luna's
  status hooks and its rollout's own turn boundaries; a helper's from its
  turn's process and stream. For every session, what its
  turns started that is still in its job, against what was there as its first
  prompt went in, whatever the CLI's version says of it (off Windows by the
  keeper's walk of the process table, a chat's CLI and all it starts carrying
  the chat's mark, so a shell that left its tree is still the chat's). The
  registry's idle overrules a hook only when it was set after that hook, a
  subagent's included. A Codex rollout's lines are told from an earlier
  process's by where the file ended as the process started, never by the
  clock; a turn dated before the start with nothing to place it is not known.
  Missing evidence is not known, never
  idle: a session just started, a Codex chat before its first turn (it holds
  the PC awake until then), a rollout lost or unreadable or one that breaks
  off mid-read, a subagent folder that will not list, a registry entry not
  surely the chat's own, a job that will not list, a helper turn that could
  not be put in its job (until nothing below it is left), a helper taken up
  after a core restart whose old job was not seen to end (one seen to end
  stays so through later restarts until its next turn). A `Stop` that comes
  after a look already counted its turn counts nothing. Keep-awake,
  the "when done" rule, `luna_session_wait` and the notices, the rows' status,
  the Delete dialog, autoDelete and the core's idle exit all read it
  ([ADR-0031](docs/adr/0031-one-lifecycle-record-per-session.md)).
- "When done" fires only once no session has been active for the whole quiet
  window, counted from the arming at the earliest, and then the minute's
  countdown, and only if a last look at that moment finds none active. Before
  that look Luna lets nothing start: agents' sends, spawns and answers are
  refused with "Luna is shutting down", no helper turn starts, and Luna types
  nothing into a pane; with anything active or any activity since the
  countdown began, it opens again and the window starts over. A key a person
  presses in a pane is activity as it comes. Every
  signal (a hook, a rollout growing, a job's processes changing, a turn count
  moving, an agent's call coming or going, a helper's turn starting or ending)
  moves one activity stamp, so a turn of seconds between two looks starts the
  window over too. Both spans run on the monotonic clock: setting the PC's time
  forward shortens nothing, and a sleep or a time change starts the window
  over. A turn waiting for an answer is active and keeps the
  PC awake, and arming the rule holds nothing awake of its own.
  `luna_power_set` returns `busy` as every session active now, of which
  `waitingForAPerson` are those waiting for an answer.
- What a turn left running (a helper's, a Codex chat's) keeps its session
  active for as long as it is there, however long it does nothing: there is
  no longer a two-hour hold after which it stopped counting. autoDelete waits
  for it. `luna_session_get`'s `leftRunning` names each by name and pid, or
  counts shells, without `active` and `quietMin`.
- A Codex turn reads as working from its rollout's start to its end; it no
  longer reads as waiting after 3 s of a still screen.
- Codex chats get Luna's status hooks (`UserPromptSubmit`, `PostToolUse`,
  `PermissionRequest`, `Stop`, `SubagentStart`, `SubagentStop`, `Interrupt`,
  `SessionEnd`), passed at each start and trusted one by one for that run by a
  `hooks.state` entry of each hook's hash, so Codex asks no review of them and
  nothing is written into the account; their URL travels in the environment,
  not on the command line, and they post by the system's curl, to loopback
  alone. A turn reads as running from its prompt, before its rollout exists,
  and as waiting while an approval is up, until that command's call answers;
  the screen is looked at only where the hooks do not deliver. The rollout
  alone ends a turn, counts it and says it ended on an error.
- A Codex chat's shells left running are seen through code mode's results
  (Codex 0.169 runs every tool call in a script): an `exec_command` result
  with a session id and no exit code, until its `CommandExecution` item
  completes, named by its command beside what the session's job holds; the
  notices name them too.
- A helper stopped by a usage limit ends with the outcome `usageLimit`, and
  its parent's notice names the limit, as for a chat: a Codex helper's from
  the message its stream ends on (a usage limit, a spend cap, a workspace out
  of credits, a quota), a Claude Code helper's from its stream's API error
  (`rate_limit`).
- Off Windows a session's job is listed (the keeper's walk), so what a
  helper's turns left keeps it active there too, and a stop between turns or a
  delete ends it.
- A program a turn started that shows a window of its own (`Start-Process
  python server.py`, a console opened for a dev server) counts as running work
  and keeps the session active; deleting or stopping still spares it on
  Windows, as it is the person's to close. A "when done" countdown whose
  read a sleep or a clock change came across does not fire: it opens again and
  the window starts over. A notice or a send Luna types into a pane is let in
  whole or refused before its first key, never pasted and then refused; a
  helper's turn that finds Luna shutting down waits with its messages and
  starts when the shutdown is called off, instead of ending with an error. A
  failure inside that last look leaves Luna able to start work again. A stop
  on a helper whose old job a restart lost track of settles it, and a Codex
  subagent whose rollout is gone once its session's turn has ended no longer
  holds the PC awake. A resumed Claude Code chat's registry entry is matched
  only while its process is running and is a terminal session's.
- Releases are signed for the updater with no Node, Tauri CLI or any other
  package's code: the release scripts open the key and compose the signature,
  and the system's OpenSSL 3 (the release runner's own, or Git for Windows'
  for `release.ps1 -Local`) derives the key's scrypt stream and signs, Ed25519
  as RFC 8032 has it, deterministic. Every signature is checked against the
  one RFC 8032 makes for the key and the message, so none can carry a nonce
  chosen to give the key away, as one made by a tampered signing library
  could; the key and its password never go on a command line, and no message
  carries anything of them. The signature, the key it is made with and
  checked against, `latest.json` and the `.sig` asset stay as they were, so
  0.19 updates onto 0.20 as before; the signature's trusted comment now also
  names the core fingerprint, dated as it is signed. The updater's endpoint
  and public key are in `src-tauri/updater.json`, which the updater and the
  release scripts both read. A new key is made with OpenSSL too
  (`release-lib.ps1` `New-UpdaterKey`), and a draft's signature checked with
  OpenSSL (`Assert-ReleaseSignature`).

### Removed

- Removed the React frontend, its build toolchain, and the npm audit, SBOM
  and license inventory. Windows releases use the Rust workspace.
- The WebView window's fit check and screenshots (`scripts/fit-check.ps1`
  with its page scripts, `scripts/screenshots.ps1` and
  `docker/screenshots.Dockerfile`): the native window's fit test and
  `scripts/gpui-shots.ps1` take their place.
- Per-place library skips and the core commands for moving helpers between chats.
- The WebView window and Tauri's side of the app: `luna --webview` and
  `LUNA_WINDOW=webview`, the fallback to the WebView window where the native
  one cannot open, its watchdog, its capabilities and Tauri plugins, the link
  opener and the attachments store of 0.19's panes, its log lines, and the
  commands only its page called and the native window has no place for (a
  library place's skip and take-in). An account's switch for agents stays
  in the app for the native window to offer. The exe
  is built with `cargo xwin build` and the setup with makensis over
  `installer.nsi` alone; the exe keeps its icon, version resource and
  statically linked Visual C++ runtime, now from the crate's own build
  script. 0.19's chat list is still imported from its WebView storage.
- The activity sampler's measurements: CPU and I/O rates of a session's
  processes, how recently it printed (and the repaint window that kept Luna's
  own resizes and focus reports out of that), and the 5-second render of every
  Codex screen that looked for an approval dialog: it is looked for only while
  a turn runs and the chat's status hooks do not deliver. The notice that a helper's
  leftovers had sat idle for 30 minutes goes with them.
- The waiting marker: a chat whose last message ended on a set line no longer
  reads as waiting, and Settings → General → Waiting for you, its "Not
  waiting" check and its recommendation are gone. 0.19's `waitingMarkerOn` and
  `waitingMarker` settings are dropped at the next save of the settings.
- **The privilege ladder** between a chat and its helpers. Luna no longer
  ranks the CLIs' modes or holds a helper to its caller's: a helper runs in the
  mode its spawn names, a chat answers any permission prompt of its own
  helpers, text that a CLI reads as a command (`/`, `!`) goes into one's own
  helpers from any mode, and a helper moves to any chat with Luna's tools and
  room for it. Settings → Agents & trust → Helpers loses **Allow helpers more
  permissions than their parent chat** and **Helpers' mode when not given**:
  a spawn that names no mode passes none, and the helper's CLI runs in what
  its own settings say (`settings.modeFrom: "cli"`). What a mode or sandbox
  lets through is the CLI's own; Luna answers only for its own surfaces (the
  hub's tokens and admission, its files, the window–core link), as the threat
  model now says ([ADR-0030](docs/adr/0030-scope-of-0-20.md), amendment of
  2026-11-20). Who may reach which session is unchanged: an agent writes to,
  answers for, stops and deletes only the sessions under it.
- The wait guard on both CLIs, the **Wait guard** box and its Settings default:
  a shell command that followed a file (`tail -f`, `Get-Content -Wait`) with no
  `timeout` is no longer refused before it runs, and the helpers' rule about it
  is gone from the guide. The hub no longer answers `/guard`.
- **Codex hooks without asking** (`--dangerously-bypass-hook-trust`), its
  per-folder choice, its box and Settings default, and the check that
  recommended it: Luna passes no bypass in any Codex start, so Codex asks for
  the trust of a project's hooks as it would started by hand, and a helper goes
  without them. Luna gave Codex no other hook, so nothing else depends on it.
- The account recommendations (the dialog, the window and the status-bar chip),
  Applied settings (**What Luna applied** and the ledger of Luna's writes into
  accounts) and the account config editor: the old window still shows these
  controls until the new one comes, and they only fail. Account files open in
  any editor. 0.19's `applied-ledger.json` is removed at the window's start;
  its settings keys for hooks without asking and the dismissed recommendations
  are passed over and dropped at the next write of `settings.json`. Settings
  sync between accounts stays.

- The CLI channels (Known-good and Latest) and the known-good list
  (`cli-known-good.json`), and the commands behind **Use**, **Mark bad**,
  **Mark good**, **Clear bad**, **Unmark good**, **Roll back** and **Keep
  it** in Settings → CLIs: until the new window comes, the old one still
  shows those controls, and they only fail. What 0.19 kept for them at each
  CLI's root (`channel`, `pinned`, `good.json`, `marks.lock`, `floor`,
  `signed-floor`) is removed at the first check; its marks bad still hold
  while the release channel names their version, and its watch over the
  version you had (one you chose to keep) is not carried over.
- Handoff, entirely ([ADR-0030](docs/adr/0030-scope-of-0-20.md)): auto-handoff
  and **Hand off now** for chats, moving a chat to another account, and
  auto-handoff for leads, on Claude Code and Codex alike. The
  `luna_session_handoff` tool is gone, and with it the `session-handoff`
  system skill (Library → System texts keeps the role texts and notes), its
  override and **Make it the override…**; a library skill of that name is
  now a skill like any other. The CLIs' own compaction stays, and so do the
  notices about a helper's context (`budgetPct`, "(context N%)", the line
  past 85%), which now always advise a fresh helper. A chat no longer starts
  with Luna's handoff folder added, and a lead no longer with its drafts
  folder. Settings, chats and helpers saved by 0.19 still load: their
  handoff fields are passed over and left out on the next write, and a
  chat's auto-handoff threshold no longer shows in `luna_session_list`.
  In the chats' and helpers' files 0.19 left, the lines its handoff and
  moves queued, in the forms it wrote them, are dropped rather than typed:
  a chat's asks, lapses and failure lines (a continuation among them), a
  lead's ask and failure, and the line a notice to a lead's parent added of
  leads that handed off (the rest of that notice still goes). A lead 0.19
  left between its two conversations goes on in the one it was leaving,
  without the fresh one's opening. A first prompt, or a Codex thread's
  generated title, that is 0.19's handoff or move opening is not taken as
  a chat's name. The `handoffs` folder 0.19 left in Luna's data folder is
  not read any more, delete it if you like; its `drafts` folders, which
  Codex's sandbox had given an access entry of its own, are removed at
  start where no link or junction is at the data folder, above it or on
  the way down to them, and otherwise left and named in the log. What the
  handoff skill left in the library's `system/` folder (its override, and
  one set aside by an interrupted **Make it the override…**, which may hold
  your own skill), whatever the case of its name, is named in the log at
  start and among the library's folders set aside, and left for you to
  move or delete. Until the new window comes, the old one still shows
  Auto-handoff, **Hand off now**, **Move to another account…** and the move
  hint after a usage limit, and they only fail.
- The library's **Needs a look**, its differences and what settled it (take
  into the library, put back, **Put all back**, **Take out**, **Keep**): until
  the new window comes, the old one still shows the list for files Luna did
  not write, and its buttons only fail, all but **Leave out**, which clears
  such a place; **Sync all** lays the library out, though its confirmation
  still says it replaces those files, which it no longer does. The bytes
  0.19 kept of every copy for the differences (`library-blobs` in Luna's
  data folder) are removed once, at the first save of 0.19's ledger. An
  account's own instructions where none of the library's go can still be
  taken into the library.
- Worktree management ([ADR-0030](docs/adr/0030-scope-of-0-20.md)): Luna
  creates, inspects and removes no worktree. Gone are the Worktrees window on
  the title bar and its sweep of worktrees no chat uses, the delete dialog's
  box that took a chat's worktree and its branch with it and its second box for
  a helper's unsaved work, the status bar's **worktree kept**, Settings →
  Chats → Worktrees (fetch origin before Claude Code makes a worktree, delete
  worktrees by default: Luna fetches nothing now), a helper's worktree (the
  `worktree` argument of `luna_session_spawn`, `worktreePending`, the queue that
  made helpers' worktrees one at a time, the `worktreeLeft` notices), the
  `dropWorktree` and `force` arguments of `luna_session_delete`, and the
  `worktree`, `branch`, `sha`, `lastCommit` and `status` fields of
  `luna_session_list` and `luna_session_get`. A call that
  still passes `worktree` to `luna_session_spawn`, or `dropWorktree` to
  `luna_session_delete`, is refused, saying that Luna does not create or remove
  worktrees: create one yourself with `git worktree add`, pass it as `folder`
  and remove it when you are done.
  A chat's **Git worktree** box stays, now only for a folder in a git
  repository: it passes `--worktree` to Claude Code or Codex, which make the
  worktree, and Luna does not remove it, with its chat or otherwise. Worktrees
  0.19 made stay on disk as they are, yours now: list them with `git worktree
  list` and remove what you do not need with `git worktree remove`. Settings,
  chats and helpers saved by 0.19 still load: the worktree fields in them are
  passed over. `luna_guide_get` now also says that implementers and reviewers that must
  outlive a turn are Luna helpers, never a CLI's own background subagents,
  which end with the caller's turn process.

### Fixed

- A chat restored from an earlier run no longer takes as its title the tag
  0.19 typed a handoff's or a move's prompt under: the native window reads
  saved titles as the WebView window did, without it.
- An update of Luna writes the chat list one last time once the sessions
  are stopped, and nothing after: a session id, a worktree or a title the
  core reports later shows but waits in memory, a move of the window is held
  back, a notice's Retry reads nothing over the list, until the update is
  called off. A rename or a helper's opening begun just before the freeze is
  waited for and lands in that write. A stop of the sessions the core does
  not answer is called off after 60 s, or from the update panel's Call off,
  instead of leaving the list frozen; an offer the host has replaced is let
  go, so the next click checks again; and the sessions' timer stops when the
  last chat to look at goes.
- While an update of Luna is stopping the sessions or handing over to the
  installer, the core cannot be restarted from the window, and the installer
  starts only if the core is still the one the sessions were stopped in;
  otherwise the update is called off with the reason.
- An update of Luna in the native window holds the core on the app's side
  too: no restart of the core is taken from the install's start until the
  installer runs, and the installer starts only if the core connected right
  then is the one the sessions were stopped in, as the stop itself reports
  it. The same core reconnected to no longer calls the update off, and a
  stop whose connection dropped before its answer is asked again of it.
  The core is checked last, right before the installer is launched, and
  no other core is connected to in between; the app does not reconnect to
  a different core while an install holds one; the window no longer calls
  the update off because it still shows the core as starting; and a stop
  of the sessions is never sent after the update was called off.
- The native window's title bar no longer pushes minimize, maximize and close
  off the window at its 900 px minimum width: below 980 px it drops its
  captions and shortens Auto-tile and the theme button, and the status bar
  shortens its CLI chips when every chip is up. A fit test opens every demo
  scene at 900x600, 1000x700 and 1280x800 and fails on a control out of
  reach; `gpui-shots.ps1 -Fit` runs it and shoots the scenes at those sizes.
- While the native window installs an update, a new or quick chat, a chat's
  deletion, a helper opened as a chat and an account's rename are refused
  with the reason instead of slipping past the chat list's last save, and
  the window's place is handed over before the install's final save.
- In the native window, an empty pane's hint wraps inside a narrow pane
  instead of running over the sidebar, and an account's name being edited
  no longer pushes its Delete button out of the account's card.
- After an update, the new window no longer finds the old core still
  counting a session and waits for you to restart it: stopping the sessions
  for the update now also ends what helpers' turns left running (a shell, a
  watcher, a build) on Windows, Claude Code's and Codex's alike, and the older
  version's core, stopped for that update, with nothing in it but such
  leftovers is replaced at once, once it has said so again as it is
  stopped. This holds for updates from 0.20 on: the outgoing
  version stops the sessions, so an update from 0.19 may still ask for a
  restart once when a helper left processes running. While a core is being
  replaced, the panes wait for the new one instead of failing.
- An update no longer installs over a session that started while the
  sessions were being stopped for it. From the moment that stop begins until
  the installer takes over, Luna starts nothing new: a chat's pane, Claude
  Code's or Codex's, that was already starting is stopped with the rest, and
  one opened then is refused with "Luna is updating"; a helper's next turn
  waits, its text kept for its sender, and a new helper is refused; notices
  and messages Luna would type into a chat wait too, while your own typing
  still goes in. If the install fails, or the stop itself fails, everything
  can start again at once; if it never happens (the administrator prompt
  declined, the PC restarted first), once the same Luna is opened again.
  This replaces the 10-minute limit after which work could start again
  during an install still waiting on that prompt, and holds across a
  restart of Luna's core meanwhile.
- Text sent to a helper whose turn was stopped before it began (by Stop, or
  by Luna for an update) is kept for its sender as undelivered rather than
  dropped.
- When a Claude Code helper's turn ends with background subagents it started
  still at work (its CLI killed, gone on an error, out of time to wait for
  them, or the turn stopped), the agent above it is told so by name, in the
  notice and in its
  wait or report: "2 background subagents ("…", "…") ended with its turn
  without finishing; their work is not done". Before, it heard that the
  helper had nothing left running and had said nothing, which read as done.
  A Codex helper's subagents cut off with its turn are told in the same
  words, also when its `codex exec` was killed or crashed mid-turn, which
  Luna missed before.

### Security

- While an update's install holds the core, it stops only for the window
  that stopped its sessions for the update, or a newer version's window
  replacing it; an uninstall meanwhile leaves it running, its folder kept.
- The scripted MCP caller a test run registers through `LUNA_DEV_TOKEN` is
  compiled into the test build alone, and so are the rights of the id it
  goes by: a release has no code that reads the variable, and a session
  given the id `dev` there no longer stands in for the person.
- A Codex approval or sandbox Luna cannot read or does not know no longer
  starts a chat with no sandbox and no approval. A `config.toml` (the
  account's or a trusted project's) that does not parse, or an approval or
  sandbox Luna does not know, now opens on `on-request` with `read-only`
  instead of Luna's stock `--dangerously-bypass-approvals-and-sandbox`, and so
  does the new-chat dialog when the core cannot answer. `on-failure` counts as
  `on-request`. A `granular` approval table is passed to Codex as it is,
  through `-c approval_policy=…`, since `--ask-for-approval` takes only
  `on-request` and `never`.
- Where nothing names a Codex approval or sandbox, a chat or a helper runs in
  **Codex's own**, from its config: Luna passes no flag and does not claim to
  know the mode, in the new-chat dialog, the title bar, `luna_session_list`
  and `luna_session_get` alike (`codex`), instead of Luna's stock bypass. A
  granular approval table or a permission profile in the config is Codex's
  own too. Only a config Luna cannot read (one that does not parse, or names
  an approval, sandbox or profile it does not know, in any layer Codex might
  read) opens on `on-request` with `read-only`, passed explicitly. Codex's
  machine-wide `config.toml` is read under the account's. A config whose
  approval is `untrusted`, which Codex 0.169 refuses to start on whatever
  flag is passed, stops a chat with an error naming the key and the file,
  where Luna is sure Codex reads it. The project layers whose
  `developer_instructions` Luna carries are those Codex trusts, a linked
  worktree by its main repository, none at or below a folder marked
  untrusted.- The Codex settings sync no longer copies the parent account's hook trust
  (`hooks.state`) into the other accounts, where it would have run hooks
  nobody there reviewed; each account keeps its own, also where the parent
  writes its hooks as one inline `hooks = { … }` table.
- A Codex helper spawned without `connectors` now has every MCP server of its
  run off, the project's `.codex/` ones and those of Codex's machine-wide
  config included, and every plugin but the library's (skills and all), where
  it used to keep the project's and plugins' servers. A server name with a dot
  no longer stops the start. One whose config Luna cannot read, or whose
  managed config sets servers, plugins or apps, is not started; the spawn's
  result no longer has `mcpLeftOn`.
- What Codex 0.169.0's Windows sandbox was seen to enforce is documented as
  an observation of Codex's, not a guarantee of Luna's: writes (`read-only`
  none, `workspace-write` the folder and temp, not `.git`), not reads, and no
  network wall: raw sockets, ssh and TLS outside schannel reach the internet
  in both. Luna still passes `windows.sandbox="unelevated"` where the account
  names none, so Codex runs commands at all, and leaves one the account names
  (`elevated`, say) as it is.

### Security

- The library's editor no longer reads or saves through a link: a skill's
  SKILL.md and an instructions file are reached from the library's folder
  with no link or junction on the way and inside the library once resolved,
  opened without following a link or waiting on a FIFO, and saved over
  nothing but a plain file; new skills and instructions likewise. A `skills/`
  planted as a junction to an account's folder could lead a save there.
- Deleting an account no longer follows a junction: the account's folder and
  its provider's folder must be no link and lie inside the accounts root, or
  nothing is deleted.
- The native updater no longer takes `latest.json`'s word, which is not
  signed, for whether an update leaves the running core and its sessions
  alone: it takes the core fingerprint from the downloaded installer's
  signature. Where the confirmation said the sessions would keep running and
  the install would now stop them (the signature names another core, or the
  core is not connected when the download is through), nothing is stopped or
  installed, and the update asks again; and an install that keeps them goes
  ahead only over the core the installer was signed for. The install goes on
  what its panel said when it was clicked: a click on a panel drawn for a core
  that has since been replaced starts nothing, and the panel shows what an
  install would do now. Retry and Install anyway, after an install that could
  not write first, no longer stop the sessions of a core that came up in the
  meantime, nor keep a word given in the old one: they ask again. A promise
  that the sessions keep running no longer stops an update when no session
  runs. No session is stopped that the panel did not count: the stop names
  the core the panel was drawn for and the sessions it agreed to stop, the
  window sends it to that core alone, and the core refuses it, stopping
  nothing, where a session started since (a chat opened, a helper an agent
  spawned, even one launching as the stop began) would be stopped too. While
  the core is not connected, or not heard from yet, the panel no longer says
  that nothing stops: it says it cannot tell, and offers no install.

## [0.19.0] - 2026-11-18

Nothing in Luna's window moves any more, so an idle window draws no frames,
and the window and the core do less as chats change: a chat waiting for you
turns orange instead of flashing. A link clicked in a terminal opens in the
system's default browser, and Settings → Links is gone. Luna is on the latest
toolchains and packages, xterm.js 6 and Tauri 2.12.6 among them, and its own
requests check certificates against the system's trust store. The terminals
no longer slow down the longer Luna runs, what waits in the core is bounded,
and a delete or the orphan sweep no longer removes a worktree another session
is in or starting into.

### Added

- A Claude Code chat can be started in auto mode: the new-chat dialog offers
  **Auto** (`--permission-mode auto`) beside Ask, Edits, Plan only and Bypass,
  and the pane's chip shows it. Claude Code then decides for itself, with its
  own review of risky actions: where that review cannot run it starts in its
  Manual mode, which asks, and an action the review cannot judge later on it
  refuses. Luna ranks such a chat where it ranks a helper in `auto`, above
  Edits and below Bypass, so its helpers run in `auto` or below; the rank
  follows the mode picked, and a start in Manual does not lower it. A
  settings file's `defaultMode: auto` now opens the dialog on Auto (it opened
  on Bypass) and starts a helper on the account's default in `auto` (it
  started in `bypassPermissions`), and a helper running in `auto` that you
  open as a chat keeps it (it took the account's default).

### Changed

- Luna builds on Rust 1.100.0, Node 26.13.1 (the current LTS) and pnpm 12.21.0,
  and its packages and crates are at their latest releases: among them
  xterm.js 6 with its WebGL and fit addons (the WebGL addon had been made for 6
  and run on 5.5), Vite 8, TypeScript 6, Tauri 2.12.6 with its plug-ins and
  jiff 1.0. Two stay behind: TypeScript on 6, as typescript-eslint does not
  support 7 yet, and sha2 0.10 and hmac 0.12, the digest stack wry still links
  into the exe.
- The terminals, a login window's too, look and type as they did: the
  scrollbar xterm.js 6 draws of its own is hidden, as the old one was, and Alt
  with an arrow key still sends what Ctrl with it sends, which xterm.js 6
  leaves to the app.
- Luna's own requests (the CLI downloads, the usage and model lookups, the
  sign-in refresh) go over the HTTP client the updater already uses, with its
  TLS, instead of a second one of their own. Certificates are now checked
  against the system's trust store rather than a list built into Luna, and the
  requests name themselves `luna/<version>`, but for three that keep the name
  they sent before: Codex's usage lookup says `codex_cli_rs`, and the lookup
  of Codex's latest release and the download of its checksums say `luna`. A
  certificate authority added to or removed from Linux's trust store reaches
  these requests when Luna restarts. The timeouts are as before, gzip is still
  asked for and undone, and no proxy is used, as before. A request that
  carries a token (the usage lookups, the model lookup, the sign-in refresh)
  follows no redirect: one comes back as the error it is.
- Nothing in Luna's window moves any more, so an idle window draws no frames:
  a chat waiting for you wears an orange title bar and status-bar field
  instead of flashing them, a working chat's dot keeps its color without
  pulsing and a background chat's is a ring, as is the status bar's dot
  beside the background count, a helper just spawned rings its chip for a few
  seconds
  without glowing, a button at work says so in words and goes gray instead of
  spinning (an account's refresh button says "Refreshing…"), and dialogs,
  menus, the sidebar and folders open and close at once.
  The terminals' cursors no longer blink unless the program in one asks for
  it: each blinking cursor redrew its terminal twice a second.
- The window does less as chats change: a change to one chat (its status,
  context, title, a handoff mark) re-renders its own row rather than the whole
  list, the status bar measures
  itself only when what it shows changes, the chat list is written to disk at
  most twice a second and without the chats' context (read again within
  seconds), and each session's output goes to its own terminal only. A new
  chat, its session id, worktree or account is still written at once, and a
  Quit or Restart window from the tray, or an update, writes what waits
  first and every change after it at once. Chats stored by an earlier Luna
  load as before.
- The window asks after all its chats in one call every few seconds rather
  than one call a chat, and the core reads a transcript's or rollout's end
  again only once the file has changed or a minute has passed.
- A session's screen is caught up with its new output rather than drawn again
  from up to 2 MB of it each time something looks at it.
- The power chip's state goes to the window only when it changes, not at
  every sample.
- The window's commands that only read files run off its main thread, and
  so do its log lines and the keystrokes it passes to the core, so a slow
  disk or a busy core no longer holds the window up. Keystrokes a core does
  not take are refused past 8 MB waiting, with an error, rather than held.
- A chat's handoff record is written to disk with the records free to be
  read, so the window's look at its chats no longer waits on that write.
- Each Codex chat's rollout is read under a lock of its own, so one chat's
  long read no longer holds up every other Codex chat's status.
- What waits in the core is bounded in bytes: a session's input (8 MB, then
  refused with an error until its CLI has taken it), its output on the way to
  the window (the reader waits, nothing is dropped), each window's queue
  (64 MB, then the window reconnects), text queued for a helper and text held
  for a person's chat (16 MB each, then a send is refused, saying why), and
  the Codex rollouts looked up by thread (1,024 kept).
- Dependabot opens no more pull requests: the crates, npm packages, pinned
  Actions and Docker images move by hand, in the project's own commits through
  the gate, and Dependabot only alerts on advisories, for the crates and at
  most the direct npm dependencies in `package.json` (GitHub reads neither the
  lockfile pnpm 12 writes nor Actions pinned by SHA; `pnpm audit` is what
  sees advisories for transitive npm packages)
  ([ADR-0029](docs/adr/0029-dependencies-move-by-hand.md)). The commit-message
  check holds every commit to the rules, with no exemption for Dependabot's.

### Removed

- Luna no longer chooses a browser for a link: a link clicked in a terminal
  opens in the system's default browser (a mail link in the mail app), and
  Shift+click does the same. Settings → Links, its default browser, its
  per-account and per-folder exceptions, the browsers found on Windows, your
  own browser command lines and a chat's remembered browser are gone. A
  settings file or a chat that still holds them loads as before, and the
  next write leaves them out.

### Fixed

- A chat terminal whose graphics context the WebView takes away (too many
  open at once) goes back to drawing without WebGL instead of staying blank,
  and closing a terminal no longer fails inside the WebGL addon.
- The terminals no longer get slower to draw the longer Luna runs: once the
  WebGL renderer's glyph atlas, shared by the terminals, had merged its pages
  (it grows with every new character and color seen), every frame of every
  terminal rebuilt all its cells; now each rebuilds once per merge. Nor does
  the atlas grow for as long as Luna runs any more: once full it starts over,
  as does its cache of drawn glyphs past 32,768 (or four times the cells of
  the largest terminal sharing it) and the colors it worked out for contrast,
  which kept the window's memory climbing with every new color seen. Luna
  carries the WebGL addon patched with xterm.js's own fixes until a release
  of the addon has them.
- Closing an account's sign-in window while its session is still starting
  ends that session once it has started, instead of leaving it running; a
  sign-in window opened again on the account meanwhile shows nothing of that
  session and starts its own only once it is gone, saying so when that takes
  over 15 s.
- The chats' status dots, and the status bar's, show in Windows' contrast
  themes, in the theme's colors, a background chat's as a ring: they were
  not drawn there at all.
- A panic in the core while it held its list of sessions no longer makes
  every later keystroke, resize, reattach and start panic too.
- A chat started again under the id of one being deleted waits for the delete
  rather than losing its registration, hooks file and Codex rollout to the
  delete's cleanup; and a worktree another session went into while the
  deleted chat was quitting is kept.
- Two saves of the chats' ties at once (`chat-ties.json`) can no longer leave
  the older of the two on disk.
- The window looks for a resting chat's saved title only under an account
  folder of Luna's, as its other commands that take one do.
- Writers at once can no longer queue more between them than a byte bound
  lets through, and one page log line longer than its 1 MB bound is written
  when nothing else waits.
- Keys a session does not take are said in its pane (`[keys not delivered:
  …]`), and the core's answer to a CLI's cursor query is never refused with
  them.
- A deleted chat is not started again by a pane that had not let go of it; a
  session killed and started again no longer hears the old one's exit; and a
  chat or helper starting in a worktree being removed with a deleted chat is
  refused, saying so, or keeps it.
- A handoff's threshold notice is queued only once its ask is saved, so the
  notifier no longer drops it as one that no ask stands for.
- A save of the chats' ties waiting behind another as the sessions are
  stopped no longer writes over what was saved before.
- Text joined to a helper's running turn on a stdin its CLI does not read
  counts against the 16 MB a helper may have waiting.
- A resting chat's saved title is looked for in the next round when a round
  stopped before it.
- A helper's delete with its worktree and the sweep of orphan worktrees keep
  a worktree a chat or helper is starting in, and refuse a start into one
  they are removing, as a chat's delete does.
- A Claude Code chat resumed into its worktree by name keeps that worktree
  from a delete or the orphan sweep from its start, not only once its CLI has
  said where it is.
- A session's exit said late, as the chat was started again, no longer marks
  the new session exited in its pane or in what agents are told: each exit
  carries its session's spawn number, and the pane takes only its own.
- Stopping the sessions waits for a save of the chats' ties under way, so
  nothing the stopping did is written after it.
- The sweep of orphan worktrees no longer offers or removes the worktree of a
  Codex chat or helper started with a new worktree before its first prompt,
  or of a Claude Code one started into an unnamed worktree, while Luna does
  not yet know which worktree is theirs.
- A chat's or helper's delete and the orphan sweep keep a worktree a session
  reached through a link or junction to its project, where they name it by
  its real path, and the other way about; on Linux a path in another case is
  no longer taken for the same worktree. The sweep does the same for a chat
  parked in a worktree and for a Claude Code session in a terminal, asks
  all of them again before each removal, and takes a path through a folder
  that no longer reads for in use.
- The sweep of orphan worktrees keeps a Codex chat's new worktree that its
  CLI made while Luna's own start waited, or while the wall clock was set
  back, by no clock at all, and asks again under its last look whether a
  session making one may have made it. The user guide says which sessions
  the sweep cannot see: Codex sessions Luna did not start, and Claude Code
  sessions started in a terminal after its last look or on a config folder
  that is neither Luna's nor the user's own.
- A session started or still running while every session is stopped for an
  update, a core leaving or the PC shutting down no longer lets the chats'
  ties be saved as the stopping left them before the process ends; they are
  saved again only once the update's install fails or the OS is seen to take
  the shutdown back, or, for an update that never came (the installer never
  started, or the window was gone mid-stop), 10 minutes after the stop. The
  window's word that the install failed, said while the sessions are still
  being stopped, takes effect only once they are.
- A session or helper started through a link to its project keeps its
  worktree from a delete by the real path even once the link is gone, and
  one that moved into a `.claude/worktrees` that is a junction elsewhere is
  found there; a start whose folder does not read is not let into a
  worktree while one is being removed, and one already on its way in has
  the removal keep the worktree, saying so; a Codex or Claude Code
  start making a new worktree keeps what its CLI may have made from the
  orphan sweep before its session is listed too. A start held off by a
  removal says why: the worktree it goes into is being removed, or a folder
  on the way does not read, so it cannot be told to be outside it.
- A Claude Code session killed long ago, whose registry entry stayed behind,
  no longer keeps worktrees from the orphan sweep; a folder that does not
  read keeps worktrees only from the sweep, not from a chat's or helper's
  delete; and starts elsewhere no longer have a delete or the sweep keep a
  worktree for want of a quiet moment to look.
- A chat's delete that keeps its worktree, another session being in it, says
  so on the status bar ("worktree kept"), and a helper's delete answers
  `worktreeLeft` with why, where both said nothing before.
- A start no longer waits for a delete or the orphan sweep reading a slow or
  dropped network drive.
- A pane or the login window whose session was killed while its reader was
  stuck is told the session ended, rather than left waiting on a dead one.

### Security

- A settings file's `defaultMode` that Luna does not offer a chat in no
  longer becomes Luna's stock `bypassPermissions`: `dontAsk` starts a helper
  on the account's default in `dontAsk` and opens the new-chat dialog on Ask,
  and a mode Luna does not know counts as `default`, which asks. A helper in
  `dontAsk` opened as a chat starts on Ask, not on the account's default.
- The new-chat dialog's Create waits until the account's settings files have
  answered, so a quick click no longer makes a Claude Code chat in Bypass, or
  a Codex one with no sandbox, before the files' own mode arrived.
- A link from a terminal is parsed before it opens and reaches the browser
  or mail app percent-encoded; one with a control character, longer than
  8 KB, a mail link asking to attach a file, or one that still holds a quote
  (the parser keeps one in a host) is refused, and the log gets
  no more of a link than the scheme of one refused, and none of the system's
  error about one.

## [0.18.5] - 2026-11-17

Luna no longer depends on chrono, which RustSec now lists as unmaintained:
it keeps its times with jiff, and reads and writes them as before.

### Changed

- Luna reads and writes its times with jiff instead of chrono, which RustSec
  lists as unmaintained (RUSTSEC-2026-0543); times are written as before, and
  `cargo-deny` no longer has to ignore the advisory.

## [0.18.4] - 2026-11-17

Luna now installs Claude Code 2.1.325 and runs nothing older. Only the
sessions above a helper send to it or answer it, and you can move a helper to
another of your chats from the window. A lead hears of its helpers while its
own background subagents run, a parent is told plainly when a helper's turn
left nothing running, and a helper's permission request reaches its parent
whole. An idle session no longer holds the PC awake when it comes into view,
asking for an account's models no longer signs it out, and an account whose
login is gone shows signed out everywhere, with Sign in.

### Security

- Claude Code below 2.1.325 no longer runs: the list's `minSafe` rises to it,
  and 2.1.323 leaves the list. 2.1.325 fixes a security issue in how
  zstd-compressed responses are decoded, and Bash permission checks where
  some compound commands ran without asking, an allow rule could run another
  program than the one it names, and some deny and ask rules did not apply;
  Luna's helpers run in `auto` and under allow rules. A version below it
  already current, held by "Use" or kept for rolling back is not started,
  the hold is dropped and the next pass installs 2.1.325; Settings refuses
  going back below it.

### Added

- The core moves a helper, with the helpers under it, to another of your
  chats on the window's word (`background_move`), never an agent's: the old
  chat is told once and owed nothing more by it, its later sends, answers,
  stops and deletes on it are refused naming the new chat, and the helper's
  next turn is told which chat it answers to. The move is refused, saying
  why, for a chat without Luna's tools, one below the helper's privilege or
  without places for it, one under which a helper would sit deeper than a
  spawn could put it, a lead or a helper, or while the helper is being
  opened, deleted or handing off. A helper's row says which chat is at the top
  of its line, which other chats wait on it, where it was moved from, and
  which chats were refused a write into it (ADR-0028).
- In the helper list each helper says who manages it and which chat its line
  hangs from, which of your other chats waits on it and where you moved it
  from; Move to chat… lists your chats, grays out the ones it may not go to with why,
  and moves it on your click; a chat refused a write into it, that it may move
  to, shows as "Move to <chat>?" under it, which opens the same dialog on that
  chat. Deleting a chat offers to pass each of its helpers to another chat, a
  chat that asked for it or waits on it picked to start with and kept; the
  moves come first, a refused one stops the delete, saying why, and a helper
  moved to another chat meanwhile is not deleted.

### Changed

- Claude Code 2.1.325 is on the known-good list, so the default channel
  installs it.
- Text and answers go into a helper only from the sessions above it: its
  chat, and a lead it works under. Another of your chats, even with
  `scope: "all"`, can still read a helper that is not its own and wait on
  it, but no longer send to it or answer its questions; it is told whose the
  helper is and that you can move the helper to it. A helper with no chat
  takes text from no agent. Your chats still send to and answer one another
  (ADR-0028).

### Fixed

- A helper's permission request reaches its parent whole: `ask.detail` in
  `luna_session_get` keeps the command (or path, or URL) up to 8 KB, where it
  was cut to 400 characters with no mark, and past 8 KB ends with `…` and how
  many characters were left out. The notice quotes only its start, marked where
  shortened and pointing to `luna_session_get`. `ask.source` says whether the
  helper itself asks or one of its subagents, where Claude Code says so. A
  Codex approval on a terminal's screen is read from every row its command is
  drawn on, down to its own options, not the first alone, and says so where the
  screen holds only part of the command. `ask.detailCut` counts the characters
  left out, and the notice shows a line break as `⏎`, not a space.
- A helper or lead that ends its turn with nothing of its own running (no
  job, subagent or helper of its own at work, no handoff of its own under
  way, nothing, a usage or context line included, on its way into it)
  no longer reads as busy: its parent's notice says "Nothing of its own is
  running: it will not go on unless you send to it", whatever its last words
  said it was waiting for, on Claude Code and Codex alike. The lead's role
  text tells it never to end a turn saying it is waiting unless something
  will wake it (a helper of its own at work; on Claude Code also a
  background task or subagent), and the guide tells parents that
  a helper's "waiting" proves nothing (#77).
- A Claude Code lead now hears of its helpers while its own background
  subagents run. Its CLI stays up for them after its answer, and Luna used to
  hold the lead's notices until they had all ended, which could take hours.
  The notice now goes in on the lead's stdin and is answered at once, for up
  to 3 hours after its answer, as long as a helper still owes it one (#73).
  One that comes while the lead's CLI is at a turn of its own goes in when
  that turn ends.
  Codex leads end with their turn, so nothing held theirs back.
- Bringing an idle session into view, moving the focus between panes, or
  any change to the panes' layout (a new chat, a split dragged, the window
  resized), no longer shows it as printing in the power chip nor holds the PC
  awake for about a minute and a half: what a session prints within a second
  of Luna resizing its terminal, or of the focus moving into or out of it, is
  taken for the CLI's redraw and no longer counts as output, nor starts a
  power-off's quiet window over. A turn at work still counts, and so does
  what a session prints in answer to keys, a paste or clicks in its terminal
  once that second has passed (a click that moves the focus starts one).
- Asking Claude Code for an account's models no longer kills the CLI while
  it may be saving a renewed token, which could sign the account out: the
  CLI renews a token close to expiry as it starts and saves the new one only
  after it has answered, and Luna killed it at the answer. Answered or not,
  it now gets up to 10 seconds to exit by itself first, what it leaves
  running is taken down after, and Luna waits for that, 12 seconds at
  most, when it quits, restarts, installs an update or is closed by
  Windows' Restart Manager. A CLI that hangs past those 10 seconds while
  saving can still be cut (#75).
- A Claude Code account whose login is gone no longer shows as signed in
  with "token refresh failed" and no way back: it shows signed out with its
  **Sign in** button, whether the CLI cleared its tokens in
  `.credentials.json` or the server turned down Luna's own refresh as
  `invalid_grant` (#76). On such a refusal Luna clears the tokens in
  `.credentials.json` as Claude Code does, under the CLI's locks and only if
  the file still holds the refused token, so helpers and moved chats no
  longer go to it, it stays signed out after a restart, and the refused
  token is not sent again; a file Luna cannot replace is left as it was, and
  its row says so. Luna no longer sends a refresh with an empty token. An
  account on hold, or any other refused refresh, stays a failed refresh. A
  Codex account with an empty token in `auth.json` reads as signed out too,
  and neither is picked for a helper; nor is a Claude Code account whose
  refresh token has run out.

## [0.18.2] - 2026-11-13

Luna now installs Codex 0.169.0, and Settings no longer offers a CLI version
below the minimum safe one: such a version says why instead.

### Changed

- Codex 0.169.0 is on the known-good list, so the default channel installs
  it.

### Fixed

- Settings → CLIs no longer offers **Use**, **Mark good** or **Roll back**
  for an installed version below the oldest safe one, which Luna refuses to
  run: such a version says "below the minimum safe version" instead.

## [0.18.1] - 2026-11-13

Luna now installs Claude Code 2.1.323 and Codex 0.168.0, and no longer runs
Claude Code below 2.1.323: that release fixes permission checks that could
let a session do more than its mode allows.

### Changed

- Claude Code 2.1.323 and Codex 0.168.0 are on the known-good list, so the
  default channel installs them.

### Security

- Claude Code below 2.1.323 no longer runs: the list's `minSafe` rises to it,
  and 2.1.319 and 2.1.322 leave the list. Before 2.1.323 a session started
  with `--permission-mode auto`, which Luna's helpers are and its sessions
  inherit, could start in a mode that allows more where auto mode was
  unavailable, and some `lsof` and `rm` commands skipped their prompts. A
  version below it already current, held by "Use" or kept for rolling back
  is not started, the hold is dropped and the next pass installs 2.1.323;
  Settings refuses going back below it.
- The `minimatch` npm package, which only the lint tools use, goes to 11 by
  a pnpm override, out of its globparts amplification DoS (vulnerable from
  7.0.0 to before 11.0.1). Nothing Luna ships carries it.

## [0.18.0] - 2026-11-13

Helpers can now lead: one spawned with Luna's tools works as a **lead**, with
its own instructions, delegates of its own and auto-handoff at 60%, while its
progress stops flooding the chat above it. Below your chats a session reaches
only its own helpers. Background subagents of a helper are no longer stopped
10 minutes after its turn, and docs/user-guide/context.md shows everything
Luna adds to a model's context.

### Added

- A handoff has a seventh section, **Skills to load first**: the skills the
  next conversation loads before its first reply, and why. The prompt that
  opens it says to load them first, and Luna's facts name the skills the
  conversation before was seen loading or reading, off its transcript
  (Claude Code's `Skill` calls that went through, the skills a person
  started with `/name`, and a `SKILL.md` read; Codex's `<skill>` blocks and a
  `SKILL.md` printed by a command its shell tool ran, the tool's result
  stating exit status 0, not one read inside a code-mode script, which the
  line says where it has none; not a skill's file the conversation also
  edited in its own folder, nor a skill loaded once the chat Luna asked
  called to hand off). A move's document
  carries the same line, the move's own ask left out, and the moved chat is
  told to load what it lists before its first reply.
- Settings → Agents & trust → Helpers: **Helpers per chat**, the places a
  chat's line of helpers shares, its helpers' helpers included (1 to 32,
  8 as before), and **Helpers' mode when not given**: the spawning chat's
  (the default) or the account's.
- `luna_session_spawn` and `luna_session_list` say how many places are in use
  on the caller's line, of how many, and how many under the caller
  (`places`). A helper refused a place hears only the sessions it can delete,
  and the chat at the top of its line is told with its next notice.
- `luna_session_get` on a helper in a worktree gives its last commit's
  subject and up to 20 lines of `git status --short`, for a parent its CLI
  keeps out of that worktree.
- A Claude Code helper may run in `auto` or `dontAsk`.
- Sessions have a role: your **chats**, **leads** (helpers started with Luna's
  tools) and **helpers**. Each is told of its role at the level of an
  instructions file, which outweighs a prompt note and survives a compaction:
  Claude Code reads it as a `CLAUDE.md` of Luna's, Codex in
  `developer_instructions` after its config's own value (ADR-0026).
- The library's **For** column takes any of the three roles. Instructions for
  some roles only reach each such session as Luna starts it, never through an
  account; several for one role all go, in name order. One that, with the
  others for its role, would not fit on a Codex command line is marked so.
- **System texts** in the library: what a lead and a helper are told of their
  role, the note after their tasks, and what a chat is told (nothing by
  default), each with Luna's own text and one override of yours, beside the
  handoff's instructions.
- `constraints` on `luna_session_spawn`: text the helper and every session
  under it are told last among their instructions, at every start and resume.
  They may only narrow the rest of a session's instructions: where they
  conflict, the stricter applies. A spawn adds to what its caller carries;
  nothing removes any. Shown in `luna_session_get`. A Codex helper you open
  as your own chat keeps them, with its role, in its conversation, and its
  **What Luna applied** says so; a new chat drops them.
- A lead whose delegates have all finished (its helpers answered, what its
  CLI ran in the background over) and that says nothing of it is woken by
  Luna, once per such end: "Your delegates have finished: …", a helper that
  answered within seconds of the lead's last turn too. Its parent hears of
  the turn that brings, what the lead wrote before included, or that it
  seems stuck if that turn says nothing.
- An answer that covers several requests to one session (a spawn and a send,
  two sends) says which is whose: requests are numbered per caller (the spawn
  1, each send one more, a `requestKey` named), the notice says it answers
  several, and the answer file, `luna_report_get` and `luna_session_wait`
  open each request's part with "Re request N".
- `expectReply: false` on `luna_session_send`: the helper's answer to it
  wakes the caller only if it opens on a status word or is a report of 400
  characters or more; it stays in the report.
- `budgetPct` on `luna_session_spawn`, and Settings → Agents & trust →
  Helpers: **Tell me when a helper's context passes** (50% unless changed, or
  never). Past it, every notice about the helper ends "(context N%)"; past
  85% a notice says so on its own, once.
- **Auto-handoff for leads** (Settings → Agents & trust → Helpers, on at 60%
  by default): a helper with Luna tools is offered `luna_session_handoff`, and
  past that share of its context, at the end of a turn, Luna asks it for a
  handoff. It goes on in a fresh conversation of its own as its turn ends, in
  the worktree it works in, with the same id, role, constraints and helpers;
  its parent hears of it with its next notice. A lead that can write no file
  goes on from a handoff Luna writes from its records. What its parent asked
  before is owed by the fresh conversation under the same request numbers,
  and notices about it give that conversation's context share. Its parent is
  not advised to replace it as its context fills.
- [What Luna adds to a model's context](docs/user-guide/context.md), linked
  from the README: every text Luna puts before a model (MCP instructions and
  tools, role texts and notes, skills, hook answers, notices, handoff and move
  texts, what Luna types in), who gets it, when, its size in tokens and how to
  change or turn it off, with totals by role. A unit test checks the page's
  sizes against the texts.

### Changed

- Luna adds less to a model's context. The MCP instructions no longer send a
  conversation to read two guide topics (~2,200 tokens) before its first
  spawn; they say where the guide is. Tool descriptions drop the lists of what
  they return (the guide has them), and the tool list is held to 9,000
  characters again. Notices drop the tails that named `luna_session_send` or
  `luna_session_stop` and "(its report picked by length)" (a read still says
  `reportGuessed`), keeping `luna_message_list` with `screen: true` only where
  a turn is quiet or stopped on a prompt Luna cannot read, and the breaker's
  line is shorter. The guard's refusal is shorter, gives one bash and one
  PowerShell way to a deadline (PowerShell's `timeout` only waits), and no
  longer names a skill Luna does not ship. The handoff's steps are said once, in
  `luna_session_handoff`'s answer: its description and the asks point there,
  and Luna's `session-handoff` instructions and `role-lead` text no longer
  repeat them.

- A lead's turns no longer wake its parent while its own helpers or its CLI's
  background work are still at it, a long interim report included, and it is
  never told "NOT done yet" of one: what it writes waits in its report for
  the notice that comes once they are done. Its answer to a send of the
  parent's still goes, and so does a message that opens on `DONE`,
  `BLOCKED`, `QUESTION`, `FAILED` or `PLAN`; questions, prompts, errors and
  exits as ever.
- `budgetTokens` on a spawn is deprecated for `budgetPct`: still taken, and
  read from saved helpers, as the share of the window it is.
- The notice that a helper has only idle processes left names what its
  transcript started them as (Claude Code's background calls, Codex's
  commands still running), not the processes alone.
- Auto-handoff ticked on a chat already past its threshold, or set lower than
  where it stands, asks as soon as the chat is at rest rather than at its
  next turn's end. Turned off before Luna typed its notice, the notice is
  not typed, even one Luna was about to type. Nor does it go in while a
  `/clear` or `/new` you just sent may still be on its way to the CLI, and
  one Luna cannot place in the conversation it asks waits without holding
  up the chat's other notices. A conversation a handoff began
  is spared until it has grown 5 points past where its first turn ended,
  however late it is first looked at: one handed off with Auto-handoff off
  and gone on since is asked once it is turned on, if it has grown so.
- A helper reaches only the sessions under it, one with Luna's tools
  included: `scope: "all"` is refused to it in every tool, as is any session
  outside its subtree (the person's chats, its parent, another chat's
  helpers), and it no longer sends desktop notifications or sets the PC's
  power. The person's own chats keep that reach
  ([ADR-0025](docs/adr/0025-a-helper-reaches-only-its-own-subtree.md)).
- A spawn that names no mode takes the spawning chat's own, or across CLIs
  the highest mode the chat's privilege covers, instead of the account's
  default; `settings.modeFrom` says which.
- The note after a task from another agent names nobody: it opens
  `[A note, not part of the task: …]`, no longer speaks of an operator, Luna or
  other sessions, and older notes still come off what a session was asked.
  Helpers are no longer started with `--append-system-prompt`.
- Library audiences of earlier versions read as roles: **Both** is every
  session, **Chats** your chats, **Helpers** leads and helpers. Instructions
  for your chats stay in the accounts with a neutral line saying whose they
  are; instructions for helpers leave the accounts and go per session.
- Instructions made every session's are refused also where the
  instructions already laid into accounts name each account they would reach
  as closely or more (`claude` and `codex` together cover `all`), not only
  where both name the same accounts: there they would reach nobody.
- **Upgrade note:** library audiences are saved so Luna 0.17 still reads the
  settings file: as **Chats** or **Helpers** where the roles match one of
  those (helpers being leads and helpers), the roles themselves beside that.
  Rolled back to 0.17, an entry for a set it has no word for (leads alone,
  helpers alone, chats with one of the two) is for every session, and
  instructions for your chats alone go into the accounts as 0.17 lays them
  out. Once 0.17 has saved the settings, 0.18 reads its words again: those
  sets come back as every session. Narrower sets do not survive a save by
  0.17; pick them again in 0.18.

### Fixed

- A handoff in a project whose `SessionStart` hook takes a while (a git
  sync) no longer gives up a minute after typing the new conversation's
  prompt while the CLI still holds it, nor types the whole prompt into it
  again once it is at work on it. The prompt counts as taken once Claude
  Code has queued it; where the settings give the CLI `SessionStart` hooks,
  the wait is up to five minutes; and a failure line whose prompt went in
  after all is dropped, the handoff recorded as through. A move's line is
  waited for the same way.
- A new Claude Code chat or helper opens on the effort Claude Code itself
  would use for its model. Claude Code keeps effort per model (`/effort`
  saves it under `modelSettings`), and the account's top-level `effortLevel`
  counts only for older models; Luna read only that one key, for every
  model. Each model now gets the level the settings files keep for it, else
  its own default, the dialog naming the file and key, and moves to it when
  another model is picked; `luna_account_get` gives `effortByModel`.
- A notice no longer says a helper "finished its turn without a reply" when
  it wrote something since its caller asked and only its last turn, on a
  notice of Luna's, wrote nothing: what it wrote comes with the notice.
- Text sent to a helper while a stop between its turns ended what it left
  running is no longer lost when the turn it was to start cannot start: its
  sender hears of it as undelivered with the next notice.

- A Claude Code helper's background subagents are no longer stopped 10
  minutes after its turn: Claude Code 2.1.319+ stops what a `-p` run left in
  the background that long after its input closed, unfinished and uncommitted
  work with it, and Luna closes a helper's input with each turn. Its turns now
  wait up to 3 hours for that work (`CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS`, in
  "What Luna applied"), not forever, so a forgotten watcher cannot hold a
  helper for good. A helper that has answered but still waits on its
  background work is told to its caller after 30 quiet minutes, naming the
  commands, Monitors and subagents it waits on and the processes in its job.
  When the CLI stops that work, the caller's notice says so instead of "it
  has said nothing", across a restart of Luna's core too.
- A Codex helper's caller hears when `codex exec` interrupted the subagents
  the helper's turn spawned as that turn ended: Codex 0.167.2 stops them
  there, their work unfinished, and exits as if all went well. Luna read the
  turn as done and said nothing of them.
- In the new-chat dialog, a model picked now opens on the effort kept for it
  even where the level shown before was one it does not take, rather than on
  its catalog default, and keeps it when another folder is picked.
- Helpers spawned side by side with `worktree: true` in one repository no
  longer fail with "unable to stat just-written file": their worktrees are
  made one at a time.

## [0.17.0] - 2026-11-13

A chat can hand off to a fresh conversation whenever you ask: **Hand off now**
in its "…" menu has the chat write its own handoff and go on from it, and
auto-handoff only asks for the same at its threshold. Moving a chat to another
account has the chat write its document too; Luna's own is used only when you
pick it or a usage limit stops the chat.

### Added

- **Hand off now** in a pane's "…" menu: any chat with Luna tools writes a
  handoff and goes on from it in a fresh conversation, with no threshold set.
  Grayed with why where the chat could not answer; the core's refusal shows
  over the pane. The ask lapses 10 minutes after Luna types it, and asks
  nothing before; one Luna could not type within 10 minutes is withdrawn,
  saying so. The menu and the chip show it as the core holds it, across a
  window reload and with the chat's CLI not running.
- **Move to another account…** has the chat write its own document by
  default, for any chat with Luna tools. Luna's document from records is
  taken only when you pick it: before the ask, while it waits (**Use Luna's
  document now**), or after a failed ask (**Build from records**). It is
  picked for you only while a usage limit holds the chat, with the window,
  its reset and how old the usage reading is.
- A failed move ask says why, and offers **Ask again** where another ask may
  go through. The preview says who wrote the document.

### Changed

- Every helper is told to install nothing and change nothing machine-wide or
  in its account unless its task names it, and to give its caller the command
  instead.
- `luna_session_handoff` is listed to every person's chat with Luna tools,
  and answers only when the chat is asked: by Hand off now, by auto-handoff's
  threshold, or by a move. Unasked, it refuses. An ask holds only for the
  conversation it was typed into, a move's ask too, and a handoff called for
  there is given up, nothing cleared, if you start another conversation
  before Luna switches. The continuation goes only into the conversation
  Luna's reset opened: if you start one of your own as Luna resets, or
  Luna's core stopped mid-switch, and Luna cannot tell the conversation the
  chat is on is the one its reset opened, the handoff is given up with only
  its copy's path said there. A failure's line that carries the
  continuation is typed only while the chat is still on that conversation,
  else as a line naming the copy. A chat answering a move's ask that was
  since decided otherwise hears that the move is over, not that nobody
  asked it.
  Every such chat gets its private drafts folder at its start.
- Auto-handoff now only asks at its threshold; turning it off closes only its
  own ask. Its ask closes when the threshold re-arms. A move of the chat,
  asked of it or built from records, closes its ask and drops its notice not
  yet typed; it asks nothing while the move is under way, preview and all,
  and asks again once the move ends with the chat not stopped (before its
  stop step, or in a move that keeps it running), if it is still past its
  threshold. Hand off now is refused for the whole move too.
- A usage limit counts as holding a chat only until every window of its
  model is read after the stop and not full, or their resets pass. A Codex
  limit is judged by its own newest reading, not by another model's, and a
  model's own limit with no reading since the stop keeps the chat held; its
  reading is kept however old, a full one days from its reset included.

### Fixed

- A move whose ask for the chat's document failed no longer switches to
  Luna's document without a word.
- A move's instructions follow the person's handoff override in the library,
  under the move's own rules.
- A usage limit scoped to one model is judged by the model of the turn it
  stopped, not the one that answered last: a Codex chat switched to Spark
  stays held by Spark's full limit. A Claude Code chat, whose failed request
  names no model, is judged by the model that answered last, a `/model`
  switch included, before the model it was started on.
- A Codex chat watching a subagent (`/subagents`) no longer has its handoff
  ask closed as if it had left its conversation.
- Auto-handoff asks again once the person's ask that took its place lapses
  or is withdrawn, once a move that took it ends with the chat not stopped,
  and once it is turned on again, rather than leaving a chat past its
  threshold unasked.
- A failure's line carrying the continuation is looked at again with your
  keys held, right before it goes in, and waits while a key of yours since
  Luna's look may not have reached the CLI yet: a `/clear` or `/new` of
  yours just before no longer lets it into the conversation you started.
- A move of a chat you began withdraws your Hand off now still open, saying
  so in the pane and on the desktop, instead of leaving it to reset the chat
  mid-move; a move is refused while a handoff of the chat is under way, and
  no handoff is scheduled while a move is.
- A move's ask fails at once, saying why, when the chat goes on in another
  conversation, instead of waiting out its ten minutes; Ask again is offered.
- Two refusals and a desktop notice about the person's handoff ask no longer
  carry a run of stray spaces.
- Luna's document from records leaves out the turns its own ask opened in the
  chat.

## [0.16.0] - 2026-11-12

A CLI version can now be marked good as well as bad, and either mark taken
back; a pane's title bar is down to the buttons used often, with the rest in
its "…" menu. Claude Code 2.1.322 and Codex 0.167.2 join the known-good list.

### Added

- **Mark good** in Settings → CLIs: an installed CLI version this Luna does
  not list becomes one the known-good channel may install, on this machine.
  **Clear bad** and **Unmark good** take a mark back, also on a version no
  longer installed, now listed under the versions: a bad mark could not be
  undone before.
- A CLI version whose first three sessions all got past their start is
  tagged "first 3 sessions ok", and the log says so once.

### Changed

- Claude Code 2.1.322 and Codex 0.167.2 are on the known-good list, so the
  default channel installs them.
- A pane's title bar keeps only the "…" menu, delete, peek, take off the
  board and "Not waiting". Rename, chat color and What Luna applied moved
  into the "…" menu, which no longer repeats the buttons beside it; a
  double-click on a chat's name renames it.
- The awake hold is logged as INFO, not as a warning.

### Fixed

- A helper turn's log line no longer holds the chat's hook and guard secret
  (a Codex start passes its guard URL in a `-c` argument), nor the helper's
  identity line whole, which made up most of the log.
- The data folder's privacy check at start no longer takes what WebView2's
  Chromium adds to its own files (denies, and a grant to its network
  sandbox's capability) for a folder open to others, so a start no longer
  warns of them and takes them away for Chromium to add back.

## [0.15.0] - 2026-11-11

A chat can move to another account of its CLI when this one runs out:
"Move to another account…" in its "…" menu hands the work over in a document
you read and may edit before it goes, written by the chat itself or put
together by Luna from its records.

### Added

- **Move to another account…** in a chat's "…" menu: the chat goes on as a
  new chat on another account of the same CLI, from a document you read,
  may edit, and send. Luna writes it from the chat's records, or a chat with
  auto-handoff on writes it itself. Folders of accounts and Luna's own are
  cut out of it, secret-shaped strings taken out, and Send goes with exactly
  the text shown. A Claude Code chat in a worktree goes on in that worktree,
  its old chat stopped first; a Codex chat in a worktree does not move.
- A pane offers the move when its account is at 90% of a usage window, or a
  usage limit stopped its chat's last turn, Codex's included (read from its
  rollout).

## [0.14.0] - 2026-11-11

A chat whose turn is over but whose subagents, jobs or helpers are still at
work now reads "background", with its own dot and count. The library refuses
to write rather than go on unlocked, and renaming an account no longer loses
what Skills & instructions knew of its copies.

### Security

- The `rustix` crate goes to 1.1.6, out of RUSTSEC-2026-0486 (its xattr
  functions marked unwritten bytes as initialized for a zero-length buffer).

### Added

- A chat has a new state, **background**: its turn is over but a subagent, a
  job its turns started or a helper it awaits is still at work. Its dot is blue
  with a slow pulse, and the status bar counts these chats and names them on
  hover. A job idle for 2 hours stops counting, as it stops keeping the PC awake.

### Changed

- A Claude chat whose background subagent works after its turn has ended no
  longer reads "working": it reads "background".
- The list of running sessions says "background:" for what runs between turns.

### Fixed

- Skills & instructions: where Luna cannot lock its library (a disk that
  cannot lock files, a lock file in the way), changes, Sync all and a system
  skill's adoption are refused with the reason instead of going ahead
  unlocked, when the window and a starting session could undo each other's
  records. Sessions still start, without their pass, and the window still
  shows the library.
- Renaming an account no longer loses what Skills & instructions knew of its
  copies: changed copies stay changed rather than turning into someone else's,
  kept copies and places left out stay as they were, instructions meant for
  chats or helpers alone keep that line Luna's to take off, and Claude Code's
  copies for listed folders move to the new name. Deleting an account forgets
  its copies and places left out.

## [0.13.0] - 2026-11-11

Sync all in Skills & instructions makes every account hold what the library
says, in one click, and Luna now keeps track of copies a grid no longer wants.
A chat's title bar shows its helpers and what they are doing, with their list
on hover, and the new-chat dialog offers the Claude models each account may
pick, straight from Claude Code.

### Changed

- Skills & instructions: **Sync all** takes the place of Sync now and makes
  every account hold what the library says, putting back deleted copies and
  replacing changed ones (after a confirmation, backed up for 30 days); places
  left out stay out. With several copies deleted, **Put all back** above Needs
  a look restores them in one click instead of one row at a time.
- Skills & instructions: a copy no longer meant for its place but changed
  there is no longer forgotten after one note. It waits under Needs a look,
  with its differences, until you **Take out** (backed up for 30 days) or
  **Keep** it as your own; **Sync all** takes every such copy out too.
- A chat's pane title bar sums up its background helpers without a click: a
  pill by the name with how many work, are idle and ask (a "!" sign, not only
  a color), and the names of those at work as far as the pane has room, asking
  ones first. The bar's settings moved to its right, past the free space,
  and the name has a maximum width.
- The helper list opens on hover, on keyboard focus or on a click or tap, from
  the pane's pill and the row's count alike, in Working and Idle tabs, and
  shows each helper's last activity and the first line of its task.
- A narrow pane folds the recolor, rename, What Luna applied and delete
  buttons into a "…" menu.
- The new-chat dialog lists the Claude models each account may pick, as
  Claude Code's own `/model` does, instead of four fixed names: a new model
  family shows up without a Luna update, older versions are under **More…**,
  and the effort buttons are the chosen model's own (none for a model that
  takes no effort). Luna reads the list from the account's Claude Code cache,
  or asks Claude Code for it in the background without starting a turn, and
  never takes another account's. Helpers spawned through Luna's tools and
  `luna_session_list` use the same list. Codex already worked this way.

### Fixed

- Skills & instructions: two copies of one name backed up in the same
  millisecond (Sync all over several accounts) no longer share a backup
  folder, the second overwriting the first. A link or junction in an
  account's folder on the way to a copy (its `skills` folder, say) is refused
  rather than followed when Luna writes, replaces or takes out a copy there.

## [0.12.0] - 2026-11-10

Auto-handoff's instructions are now a skill Luna serves only to the chat that
turned it on, and the library's new System skills section lets you override
them. A Claude account can be signed in from Luna on Windows again, usage shows
only the windows the provider sent (no more "5 hours" at 0%), every dialog fits
a short screen, and the Wait guard has a default in Settings and is kept per
folder.

### Security

- The core keeps no credential. Through 0.11 every command to the core took
  a token from `core.json`, a file every process of yours could read, a CLI's
  sandbox included; on Windows a sandboxed command was refused at the core's
  loopback port, but the same command reached the core through any plain
  loopback forwarder (measured with Codex 0.167.0). The core now listens on an
  endpoint of its own, a named pipe on Windows and a socket in Luna's private
  `core/` folder elsewhere, and lets in only your processes that the OS marks
  unconfined: Codex's Windows sandbox is refused the pipe's open, and on Linux a
  process in another user namespace (Codex's bubblewrap sandbox), with
  `no_new_privs` set, or under more seccomp filters than the core is turned
  away. `core.json` holds the core's pid and the
  endpoint's name, nothing that lets anyone in
  ([ADR-0021](docs/adr/0021-core-endpoint-by-os-identity.md)).
- A core of 0.11 still running when this version starts is asked how many
  sessions it runs and, with none, to stop, as a core of another build always
  was; with sessions the banner asks you to restart it.
- Only a chat with Auto-handoff on is told of it. The agents' guide no longer
  has a Handoff topic or section, which every session with Luna tools could
  read, a chat's settings as another session reads them (`luna_session_get`, a
  listing) no longer show its threshold, and a session not offered
  `luna_session_handoff` that calls it all the same is answered as for a tool
  there is not. A session that reads the chat's conversation with
  `scope: "all"` still sees the notices in it, as it sees all of it.
- A chat writes its handoff in a private folder of its own in Luna's data
  folder, not the system's temp folder, which others may share; a Codex chat
  is given that folder to write in (`--add-dir`) where its sandbox writes at
  all, and one under a read-only sandbox is told it can write no file. Where
  a start cannot make that folder private, the chat is given and named none
  and writes its handoff outside Luna's folder; What Luna applied says why.
- Auto-handoff's records and copies in Luna's data folder are kept from
  anyone else who can write there (a data folder others share). Through 0.11
  `handoffs.json` was read through whatever link stood at its name, and a link
  planted at the `handoffs` folder or a chat's folder took the records and
  copies to a folder another user holds. Each folder on the way is now checked
  from the top down, a private folder of yours with no link or junction, before
  anything below it is read, written or given to a CLI; the records and a copy
  are read only as private files of yours, by their handle; and a switch, a
  fresh one or one a core restart took up, types nothing until the copy it
  points the next conversation at is the one on record (its SHA-256).
- The library's override of the handoff's instructions is served only as a
  file of your own making, not a second name of another file nor another
  user's file, and is read alone and no further than its 64 KB: nothing beside
  it in its folder, a FIFO or a large file, can hold up or swell the read.
- A skill of yours in the library named `session-handoff` is laid out nowhere,
  and a copy of it laid out before is taken out unless changed there since: in
  an account it would tell every session there of the handoff. Its row in the
  window says so and offers to make it the handoff's override; no new skill
  takes the name.

### Added

- The Skills & instructions window has a **System skills** section, apart from
  your skills, for the handoff's instructions: whose text chats are served
  (Luna's own with its version and hash, yours with its path, or yours that
  will not do and why), **Edit** (your override, starting from Luna's text),
  **Compare with Luna's** and **Back to Luna's** (your override into Luna's
  backups). It has no accounts, audience or folders: one override for everyone.
  Where the backups are on another drive and a save changes your override
  while it is copied there, it is kept aside in the library's `system/` folder
  instead, not served, and the section names where.

### Changed

- Auto-handoff's instructions are served by its own tool: `luna_session_handoff`
  without a file answers how to hand off, where the file goes, what Luna adds
  by itself, and the instructions for the file, Luna's own (built in) or one
  override of yours for every chat, `system/session-handoff/SKILL.md` in the
  library. One that will not do is not used, and the chat is told why. The
  first notice sends the chat to the tool; the firm one, near where the CLI
  compacts, carries the file's headings itself
  ([ADR-0022](docs/adr/0022-content-luna-serves-at-its-own-trigger.md)).
- A chat whose CLI compacted its conversation before a handoff, or that went on
  with a model of a larger window, is asked again once past its threshold, and
  its title bar chip reads **handoff N% · asked**, saying when in its tooltip;
  only a fall in tokens counts as a compaction there.
- The chat's What Luna applied panel says whose handoff instructions it gets,
  with Luna's version or the override's path and the text's hash, and warns of
  a `session-handoff` skill in its folder or account, which Luna never uses.
- The next conversation's prompt is made as it is typed, after the reset, not
  before; it names each helper's model, effort and transcript, quotes names and
  tasks as data, and says that approvals the handoff quotes hold as far as the
  user's quoted words go and no further. A file written to Luna's own
  instructions that lacks one of their headings is named in the answer, not
  refused. Each handoff on record keeps which instructions it was written by,
  the firm notice's checklist where that was all the chat had.

### Fixed

- The new-chat dialog's Wait guard started off every time, with no default in
  Settings and nothing kept for a folder. It now starts like the other boxes:
  from Settings → Chats (off unless you turn it on), or from the folder's own
  start, which "keep for this folder" now keeps it in, "back to …" restores and
  picking another folder resets. Ctrl+Shift+N takes the same start. On Codex
  the box shows off, and the chat starts without it, unless hooks run unasked.
  A folder's start kept by an earlier version leaves the guard to Settings.
- A usage window the provider did not send showed as 0%. Claude's usage
  endpoint stopped sending the 5-hour session window, and every Claude account
  then had a "5 hours" window at 0% with no reset, in the Accounts window, the
  status bar and what `luna_account_list` and `luna_account_get` give agents.
  A window that is not sent is now left out everywhere, the account choice of a
  spawn given a list included, which judges an account by the windows it has.
  A Codex answer with no window left no longer keeps the last ones on show,
  and neither the CLI's cache nor a Codex rollout stands in any more for a
  newer answer from Luna, which brought back a window that answer no longer
  had.
- A Claude usage row of a kind Luna did not know was dropped without a trace,
  so a new or renamed window vanished. It is now shown under the server's own
  name for it with its kind, "monthly (monthly_all)", and its kind is logged
  once. A Codex window of a length other
  than five hours or a week was already shown, named by its length; "1 day"
  and "1 hour" no longer read "1 days" and "1 hours".
- Signing a Claude Code account in from Luna works on Windows. The login
  window's session wrote its settings to `hooks\login:claude:<name>.json`, a
  name Windows refuses, and never started. A login now writes no file at all
  (its one setting, the core-folder deny, goes on the command line), and every
  file Luna names for a session spells the session's id as a name every
  volume takes. Codex's login wrote nothing of the kind and was not affected.
- A chat is no longer started in a folder its CLI cannot run in. A folder that
  no longer exists, or a Git worktree asked for outside a git repository (where
  Claude Code and Codex both exit at once), is said in the New chat dialog
  before Create, in the pane of a chat whose folder has gone, and to an agent
  spawning a helper there. Claude Code's `WorktreeCreate` hook, which makes a
  worktree without git, is let through when the account's or the project's
  settings name it; where it could come from a source Luna does not read (an
  enabled plugin, managed settings, a settings file or policy key it could not
  read), the dialog only warns.
- The CLI canary no longer blames a new Claude Code or Codex version for an
  exit that was not its doing. A session that ends on a cause outside the CLI
  (a worktree asked for outside a git repository) or that you or Luna stopped
  (a chat deleted or restarted, the sign-in window closed, a helper stopped)
  is not counted, and the next session is watched in its place. On the
  0.11.0 test build two chats in a folder outside git raised "2 of the 2
  sessions ... failed soon after starting" for Claude Code 2.1.319.
- On a short screen (1024×600, or a laptop at 125% or 150%) the bottom of New
  chat was cut off with no way to scroll to it, Create and Cancel included; at
  150% so were the buttons of a long confirmation (an update's notes with
  sessions running) and the bottom of the sign-in window. Every dialog now
  fits the window: its title bar and its buttons stay in view and the part
  between them scrolls. A long menu or popover (a chat's helpers, an
  exception's accounts, the browsers a link can open in) scrolls rather than
  run off the window, and one closes once a scroll moves its button away or
  the window is resized rather than stay over the wrong row. New chat's account list opens over the
  dialog, below its box or above it, wherever there is more room.
- In a chat's list of helpers, each helper's Delete stood past the list's
  right edge and "Open as chat" broke onto two lines; a helper's four buttons
  now fit its row, each label on one line.
- A Codex helper with Luna's tools was offered `luna_permission` beside them,
  a tool only a Claude Code helper's CLI calls (its permission prompts), so
  its model paid for a tool it must never use and could call it by mistake,
  which held an ask for its parent to answer. Only a background Claude Code
  helper is offered it now; from anyone else a call to it is refused as an
  unknown tool.

## [0.11.0] - 2026-11-09

Settings is a dialog of pages, each CLI follows a known-good or latest channel
you can roll back, a chat can hand itself off to a fresh conversation as its
context fills, and Luna changes nothing in a CLI's behavior without asking and
shows what it applied. Helpers always run in the background, and a library
skill or instructions file can be kept to your own chats or to helpers.
Breaking for agents: Luna's tools are renamed `luna_<resource>_<verb>`
(`luna_spawn` is `luna_session_spawn`, and so on), the old names only answer
with the new list, and visible helpers are gone.

### Breaking (for agents)

- Luna's MCP tools are named for the thing they act on and what they do,
  `luna_<resource>_<verb>`, sixteen of them: `luna_session_list`,
  `luna_session_get`, `luna_session_spawn`, `luna_session_send`,
  `luna_session_answer`, `luna_session_wait`, `luna_session_stop`,
  `luna_session_delete`, `luna_message_list`, `luna_report_get`,
  `luna_account_list`, `luna_account_get`, `luna_run_list`, `luna_guide_get`,
  `luna_notification_send` and `luna_power_set`. The old names (`luna_list`,
  `luna_sessions`, `luna_spawn`, `luna_send`, `luna_answer`, `luna_read`,
  `luna_wait`, `luna_kill`, `luna_delete`, `luna_guide`, `luna_notify`,
  `luna_power`) are gone: called, one answers with the tools there are now. A
  session already running keeps the old list until its CLI reconnects.
- A listing is compact rows, a page at a time, filtered before anything costly
  is read (`luna_session_list`, about 60-80 tokens a row); one session whole is
  `luna_session_get`; accounts are `luna_account_list` and `luna_account_get`;
  past spawns are `luna_run_list`, compact and limited.
- `luna_session_wait` returns the report by default, with how many other
  messages there are and a file of all of them (`reply: "all"` or `"none"`
  for the rest), and the terminal only with `screen: true`. `luna_report_get`
  gives the report alone, starts the helper's autoDelete count over, and with
  `ack: true` collects the answer so no notice follows.
- Arguments say their units: `timeoutSec`, `quietSec`; a read's left-out count
  is `rest`, its tool-call markers `toolCalls`; `luna_session_delete` takes
  `id` or `ids` and answers `already` for one deleted before.
- `luna_guide_get` takes a topic from a fixed list, the tools' names and
  `notices`, `restart`, `helpers`, and with none gives a short index. The
  instructions carry a short recipe for a safe spawn and name the two topics
  to read first; nobody is asked to read the whole guide any more.

### Added

- **Auto-handoff** for a chat with Luna tools, ticked in the new-chat dialog
  or the chat's What Luna applied panel with a threshold (70% of the context
  unless changed). Past it, at a turn's end, Luna asks the chat for a handoff
  file, with the steps and its sections, and a firmer second time near where
  the CLI would compact. On the chat's call of `luna_session_handoff`, offered
  to that chat alone, Luna copies the file, waits for the chat to be at rest
  (a draft holds it), types `/clear` (Claude Code) or `/new` (Codex) and then a
  prompt pointing the new conversation at the copy, with the facts Luna knows:
  helpers and their state, branches, answers owed, text not yet delivered,
  accounts, the chat's settings. Helpers, debts and notices carry on; the chat
  keeps its row, and a divider over its terminal opens the handoff and the
  conversation before. A switch a core restart cuts off is taken up where it
  stood; one that fails leaves the old conversation going, with a notice. See
  docs/user-guide/chats.md and ADR-0020.
- Each skill and instructions file in the library is for your own chats, for
  helpers, or for both (the default), picked in the Skills & instructions
  window's new **For** column. Claude Code sessions of the other kind are
  started without it (`claudeMdExcludes`, `skillOverrides`); Codex skills are
  turned off per run, and since Codex reads its account's `AGENTS.md` in every
  session, instructions for one kind are laid out with a line saying whose
  they are. What Luna applied lists what the audience kept from a session.
  See docs/user-guide/library.md.
- Luna's note to every helper also tells it what it still owes: to stop, before
  its turn ends, the background work it started that nothing needs any more
  (watchers, wait loops, dev servers; not helpers it waits on, nor what its
  task keeps running), keep each caveat beside the claim it limits, and send,
  push or post nothing outward unless its task asks.
- "What Luna applied" (the ⓘ button on a chat's title bar): everything Luna
  added to the chat's CLI at its latest start, grouped as your choice,
  runtime, boundary or workaround, each with its value and why, and what Luna
  wrote into the account (the folder's trust, the library's copies). Tokens
  and Luna's local addresses are never shown. A helper kept to Luna's MCP
  server and with its auto-memory off shows both as boundary items.
- **Watch** in a chat's helper list shows that helper's conversation,
  read-only and live: what its agent asked and what it said, tool calls marked,
  updating every second while it works. A long conversation shows its latest
  part, without very long entries such as a tool's whole output, and says so.
- Each CLI follows a channel, chosen in the new Settings → CLIs: **Known-good**
  (the default) installs the newest version this Luna release lists as checked
  with it (`src-tauri/cli-known-good.json`, which also names the oldest version
  allowed to run); **Latest** installs each new release, as before. A new
  version is run once offline before it is used (its version, every flag Luna
  passes in each command's help, Codex's hooks config) and marked bad if it
  fails. The first three sessions on a new version are watched; two that fail
  soon after starting bring a notice, a status-bar chip and **Roll back** in
  Settings. Nothing rolls back on its own. See docs/user-guide/updates.md and
  ADR-0019.
- Settings → CLIs lists the installed versions of each CLI, tagged current,
  previous, known-good, in use or bad, with **Use** to go back to one (its
  files checked against their checksums first, and held there until you
  follow the channel again) and **Mark bad**.
- `luna_session_get` shows the CLI version a session's process last started
  from (`cliVersion`).

### Changed

- The version before the current CLI is always kept, and so is any version a
  session still runs on: an update no longer sweeps them, which on Linux and
  macOS could delete a running Codex session's folder. Each Codex install now
  records every file's checksum, not only its size.
- The floor below which no CLI version is downloaded rises when a version
  becomes current rather than when it is downloaded, and Codex has one too.
- Settings → Updates shows Luna and the core; the CLIs moved to their own page.
- Settings is a wide dialog with its pages listed on the left (General, Chats,
  Agents & trust, Links, CLIs, Updates) and only the page scrolling, in place of one
  narrow column of ten sections. Every setting is a row with its control on
  the right and one line on what it does; what a change costs (helpers above
  their chat, a deleted worktree's uncommitted work, accounts moved by hand)
  stays in view, and the rest of each explanation folds into a "How … works"
  line. Help text is 12 px at the least and passes WCAG AA in both themes.
  Lists say when they are empty, link exceptions are added from a menu, and a
  browser's long command line can be shown in full or copied. The page list
  and the exception menu work from the keyboard, Tab stays inside the dialog
  and the focus returns to what opened it, and a draft on one page survives a
  look at another. See docs/user-guide/settings.md.
- Confirmation dialogs (deleting a chat, an update, a library change) take
  the focus when they open, keep Tab inside them, leave the window or dialog
  behind them unreachable until they close, and then give the focus back to
  the button that opened them. Enter on a focused Cancel now only cancels.
- Luna is a runtime, not a provider configurator (ADR-0016): it adds to a CLI
  what running it needs and what protects its own boundary, changes nothing
  else without the person's say, and shows what it applied.
  - The guard against endless waits is off in chats unless "Wait guard" is
    ticked in the new-chat dialog; helpers keep it. Chats with Luna's tools no
    longer get it by default.
  - The new-chat dialog asks "Trust this folder in <account>", unticked;
    unticked, nothing is written and the CLI asks itself. A chat an agent
    starts never writes trust (a Codex helper is trusted for its own runs).
    Quick chat opens the dialog for a folder the account has not trusted.
  - Codex: `approvals_reviewer="user"` only for helpers; `windows.sandbox` only
    when the account's `config.toml` names none;
    `check_for_update_on_startup=false` passed on every start and no longer
    written into new accounts (lines already there stay); a helper gets
    `--skip-git-repo-check` only outside a Git repository.
  - Claude Code: with the library's instructions, the other folders added to a
    session (Luna's notes folder, the library's skills folders, the user's own
    `additionalDirectories`) are no longer read as memory.
  - Chats that had the guard because of Luna tools are named in a notice
    once; a chat's ⓘ panel turns the guard on for it.
- **Breaking for agents:** every helper runs in the background. A Codex
  helper's `approval` is `never` (the default) or `auto-review`; one that asks
  a person (`on-request` and the like) is refused, as nobody is there to ask.
  `luna_session_spawn` answers with `id`, `settings`, `turnsEnded`, `sameFolder`, the
  worktree and `accountChoice`; `started`, `screen`, `ask`, `starting` and
  `warnings` are gone with the visible start that filled them.
- **Breaking for agents:** "Open as chat" hands a helper over to you for good.
  It becomes a chat of your own, a top-level row seated in a pane that keeps
  the wait guard until you turn it off: its agent can no longer send to it,
  wait on it, read it as its own, stop or delete it, and is owed nothing more
  by it. The agent hears so once, in a notice that outlives a restart. Text
  still queued for the helper is not delivered, and whoever sent it is told
  where it is. Luna asks before it does this, and while it opens the agent can
  neither stop nor delete it. An opening Luna cannot finish (the window closed
  midway and stays closed) gives the helper back to its agent after 15
  minutes, and the agent is told.
- Deleting a chat lists the helpers its agent runs with what each is doing and
  what its worktree holds. Ticking the box to take them along never stops one
  still at work, or drops a worktree with uncommitted work, by itself: each
  takes a box of its own, and Luna checks both again as it deletes. A helper's
  own Delete waits while it works and keeps such a worktree.
- Chats an earlier Luna ran as visible helpers are your own chats: their rows
  are no longer nested under the chat that spawned them, no agent can stop,
  delete or answer for them, and the guard notice names them too, as they now
  start without it.

### Removed

- **Breaking for agents:** `luna_session_spawn`'s `visible`; a call passing it is
  refused as any unknown argument is. With it go visible helpers, the chats an
  agent started in the window, and what kept them: the window's part in a
  spawn, carrying them on after a core restart, their nesting and reparenting,
  and the rule that one you typed into was never auto-deleted.
- **Breaking for agents:** `luna_session_answer`'s `keys` (raw keystrokes for a
  helper's screen); a helper has none, and a dialog in your own chat is yours.

### Security

- A Codex helper spawned without `connectors` is not started when its
  account's `config.toml` has an MCP server whose name Codex cannot turn off
  for one run (a dotted name such as `team.slack`): such a server stayed on
  while "What Luna applied" said the helper had Luna's server alone. The
  error names the server; rename it, or spawn with `connectors: true`. The
  servers a Codex helper keeps all the same, those of the project's
  `.codex/config.toml` and the plugins on in the account's and project's
  config as Codex merges them, are listed in
  the spawn's result (`mcpLeftOn`) and in "What Luna applied".
- Luna starts no CLI but its own managed copy. Before its first download had
  finished, or with that copy damaged, a session ran whatever `claude` or
  `codex` the system found on PATH, past the checks a managed version passes
  (its signature or checksums, the probe, the known-good list and its
  minimum). Now such a session is refused, saying whether the CLI is not
  installed yet or its copy is damaged; Settings → CLIs installs it again.

### Fixed

- A session starting while the update pass removed an old CLI version could,
  once that version was installed again under the same folder name, run from
  it under a lock no later pass saw, and on macOS and Linux a later pass could
  remove the folder it ran from. The lock files under `in-use/` now stay.
- A release that changed only the CLI policy (`cli-known-good.json`, its
  minimum safe versions) kept the core's fingerprint, so its window attached
  to the old core still running, which went on allowing the old minimum. The
  policy is now part of the fingerprint, and a test holds every file compiled
  into the core to it.
- A Codex helper spawned with `approval: "auto-review"` failed every turn
  after its first: `codex exec resume` refuses `--approve-for-me`, so its
  later turns pass the three settings that flag stands for instead.

## [0.10.2] - 2026-11-08

A security release for the agent tools and the app window: an agent reaches
only its own helpers unless it asks for every session, helpers start without
their account's connectors, the window's page can call only what it uses, and
no CLI runs with administrator rights. It also makes Luna's tools cheaper for
agents to use and fixes several ways a helper's report or a transcript was
misread.

### Security

- An agent can interrupt only its own helpers and theirs: `luna_send` with
  `interrupt: true` reached any session, so one agent could stop another
  chat's helper mid-turn, or a person's chat, and dismiss the question it was
  waiting on.
- Reading, waiting on, sending text to and answering a session that is not
  the agent's own takes `scope: "all"` in the call, and text so sent, an
  answer typed in as a message too, arrives opened `[From session <id> via
  Luna]` rather than as the user's own words.
- `luna_list` and `luna_sessions` list any session's state but give another
  session's question, permission request or task, and others' past spawns
  with their prompts and replies, only with `scope: "all"`.
- Helpers start with Luna's MCP server alone: no MCP servers or connectors of
  their account (claude.ai's Gmail, Slack and other tools that send and write
  in the person's name; Codex's apps and its account's `config.toml`
  servers), unless spawned with `connectors: true`, and Claude Code helpers
  with auto-memory off. A Codex helper keeps the servers of the project's
  `.codex/` and of plugins, which Codex gives no way to turn off for a run.
- Every `luna_*` tool refuses an argument it does not take, or of the wrong
  type, naming those it takes, instead of running on a default: `read_only`
  for `readOnly` spawned a helper that could write.
- The window's page reaches only what it calls, so a script injected into it
  would get no more: each app command is granted to the main window by name
  and refused to any other window or a remote page, plugins are cut to the
  calls the page makes (no menus, trays, new webviews or paths), the bridge to
  the core passes on only the 35 commands the page sends, each on its own lane
  (never the core's `shutdown`), and account and folder paths are checked
  where they come in: an account path must be an account folder Luna lists,
  reached through no link or junction and spelled as on disk, a folder a
  plain absolute path.
- Turning on Settings → Helpers → "Let helpers run with more privilege than
  the chat that spawns them" asks again in a system dialog, which the page's
  own calls cannot answer, and a yes counts only for the state it was asked
  about.
- No CLI runs with administrator rights. Luna started as administrator starts
  its core, and so every session and helper, with the desktop's ordinary
  rights and the user's own environment, not the elevated one's; where there
  are none (no Explorer, another account's credentials, User Account Control
  off) it starts no session and says to restart it normally. The core refuses
  sessions and helpers whenever it runs elevated, and a core an older Luna
  left running elevated is replaced or, with sessions, restarted on the
  banner's button; one whose rights cannot be read counts as elevated. An
  elevated core lets an ordinary window in only to stop it, past the check
  that otherwise turns away a client of lower integrity; a sandboxed one is
  still refused. Root is refused the same way on Linux and macOS, and the
  live tests refuse to run from an elevated terminal.
- The transcript readers, which take what a prompt-injected agent can write,
  are fuzzed: each provider's are pure functions of a file's bytes, held to
  golden readings of the fixtures, with a cargo-fuzz target each
  (`scripts/fuzz.ps1`) that also checks every text read comes out of the
  input, and that a line's length or depth costs no more than its bytes. A
  scheduled workflow runs them weekly.

### Changed

- `luna_list` names each account with its defaults and usage but no longer its
  models, Codex's catalog with every model's levels being most of what a
  listing cost an agent that lists often; `models: true` brings them back.
- A notice says which lines of its answer file are the report (`lines 1-N of
  M`), and `luna_read` with `report: true` gives the report alone, so an agent
  that wants only the report need not read everything else the helper said.
- `luna_guide` takes a `topic`: a word of a section's title gives that section,
  `all` the whole guide, and no topic its opening with the sections to ask
  for. The instruction sessions get says to read it whole before the first
  spawn; a lookup after that costs a section, not the whole manual.
- A notice ready for an agent that another of its helpers is still working for
  waits up to 20 seconds for that one's answer, so helpers finishing within
  seconds of each other wake the agent once, not once each.

### Fixed

- A Codex prompt that opens with a tag of its own, such as `<task>…</task>`,
  is a prompt again: `luna_read` and answers left out every user message
  starting with `<`, and with it the start of the turn that answered it. Only
  the blocks Codex itself puts there (its environment, skills, shell commands,
  interruptions and the like) are left out.
- `luna_answer` refuses answers to a question with no dialog that are one too
  few or too many, rather than send them as if they answered it.
- A command call whose timeout is past any clock (a Claude Code `Bash`,
  `PowerShell` or `Monitor` call, a Codex call's bound) made the time it is due
  wrap round to one long before the call, so a caller waiting on a quiet turn
  heard of it as hung without the call's own deadline holding the notice back;
  it is now due never, as an open call with a long timeout is. Codex token
  counts past 2^64 no longer overflow the total the answering model is found
  by. Both found by fuzzing.
- A Claude Code session's answer to a prompt the CLI put in itself (a
  background task's end) opens a turn of its own for a caller reading the
  conversation on from a cursor too, not only for one reading from the top,
  so the report it leads with is the newest when that prompt and its answer
  land in different reads.
- A transcript line that is not UTF-8 no longer ends the search for a chat's
  opening prompt (the name it goes by until the CLI titles it), in Claude Code
  and Codex alike: it is read as the conversation reads it, a stray byte as
  U+FFFD.
- A helper's notice led with the wrong report when its CLI started a short turn
  of its own after the real one (a background job's end it took up): the short
  line came first and the long report sank among the rest. Such a turn now
  leaves the report to the long message before it, and comes next as the last
  word.
- Helpers followed rules their account's instructions set for the person's own
  chat — one wrote its progress notes in the language set for "the operator".
  The note a helper gets now names the words such instructions use (the
  operator, the user, the person at this machine) and tells it to answer its
  caller in its task's language and form; a Claude Code helper gets the note
  after its first prompt too, not only in its system prompt.

## [0.10.1] - 2026-11-08

A security and reliability release; it also carries the changes of 0.10.0,
which was never published. Claude Code downloads are checked against
Anthropic's signed release, sessions are kept away from the key to Luna's
core, an interrupted CLI install is no longer mistaken for a finished one,
shutting the PC down or restarting it when done works again, and the
updater's signing key is kept out of the release build.

### Security

- Dependencies with published vulnerabilities are updated: rustls 0.23.46 and
  rustls-webpki 0.103.16 (TLS handshake fixes), webpki-roots 1.0.11 (distrusted
  emSign roots removed), zip 9 under the updater (tauri-plugin-updater
  2.14.1) and tokio 1.53.3 (an unsound `AsyncFd` constructor); among the build
  tools, Vite 7.3.7 and source-map-js 1.2.2.
- Every pull request, every change to master and a weekly run check the
  dependencies for advisories, licenses and sources (cargo-deny, `pnpm audit`),
  the history for committed secrets (gitleaks), and the code and workflows with
  CodeQL. `scripts/supply-chain.ps1` runs the same checks locally.
- The updater's signing key no longer reaches the release build. `tauri build`
  runs every dependency's build scripts, proc-macros and frontend tooling, and
  any of them could have read the key it was handed; the installer is now
  built unsigned and signed afterwards in a job of its own, which runs only the
  Tauri CLI pinned by `pnpm-lock.yaml`, without network, and checks the
  signature against the updater's public key before publishing it. The release
  still takes one approval, now given to the signing job. `release.ps1 -Local`
  signs the same way, builds an export of the commit rather than the checkout
  it signs from, and no longer runs the built exe to ask its core fingerprint.
  A key rotation's bridge release is checked against the old public key, given
  as `UPDATER_TRUSTED_PUBKEY`.
- Claude Code's release manifest is verified against its detached OpenPGP
  signature and Anthropic's release key, pinned in Luna, before the exe's
  checksum from it is trusted: whoever serves the download bucket can no
  longer hand Luna an exe of their own with a matching manifest. Codex is
  still checked only against the checksums of its own release.
- The signed manifest must name the version it was fetched for, and no Claude
  Code version below the highest signed one Luna has accepted (or below
  2.1.319 on a fresh install) is installed, so an old signed release cannot be
  replayed under the unsigned `latest` pointer. A signature is held to its
  signed creation and expiration times, and the release key to its expiry
  where the signature gives one.
- Sessions are denied the folder that holds the key to Luna's core, wherever
  their CLI can enforce it: Claude Code's file tools everywhere, and Codex's
  `read-only` and `read-only-net` sandboxes on Linux and macOS. A Claude Code
  session whose deny cannot be written does not start, rather than run without
  it.
- On Windows, where a Codex sandbox can still read that file, the core instead
  refuses the connection: it checks which process is at the other end of each
  loopback connection and turns away one running in a CLI sandbox (a restricted
  or contained token, or a lower integrity level) before it can present the
  key. A normal window is unaffected. Codex's `workspace-write` on Linux and
  macOS remains able to read the file; the threat model (F3) says why.

### Added

- The installer puts `THIRD-PARTY-NOTICES.txt` beside Luna, and each release
  publishes it too: the license texts of every crate and npm package Luna is
  built from, and where their source is; the Rust standard library's own
  inventory; and what the build tools and the installer add: Vite's helpers,
  NSIS and its Tauri plug-in, the WebView2 loader and the Visual C++ runtime.

### Changed

- `luna_power` disarms with the action `cancel`; `off`, its old name, still
  works.
- Release notes are made from this file: a release's notes are its version's
  section, the sections of every version since the last published release, a
  link to the full changes, and the commands and checksums to verify the
  download. The tag's message no longer carries them, and the update chip
  shows the section's opening paragraph.
- Commit messages follow one written-down form (CONTRIBUTING.md, "Commits"):
  a sentence of at most 72 characters saying what the change does, no type
  prefix, a blank line, then why. The commit-msg hook, the gate and CI refuse
  the rest; past commits stay as they are. A refusal says to commit again from
  the message, not to amend, and `amend!` commits are let through to be
  squashed, as `fixup!` ones are.
- Dependabot opens its grouped update PRs monthly instead of weekly, and leaves a
  new Node or Rust major to a hand-made migration.
- Tauri 2.12.5 and its plugins' latest releases; base64, dirs, getrandom and
  toml_edit moved to versions the rest of the tree already carried, which drops
  dirs 6 and toml_edit 0.22 from the build; vt100 0.18 for the screen a helper's
  output is replayed onto.

### Fixed

- A CLI install cut short (Luna killed or the PC off while Codex's package was
  unpacked) could be taken for a finished one on the next check: the executable
  being there was enough, so Codex ran without its sandbox helpers or from a
  truncated file. Both CLIs are now fetched into a staging folder, checked file
  by file against what their release publishes (Claude Code: sha256 and size;
  Codex: the package's sha256, then each file's size), given a completion
  record, and moved into place with one rename under a name no folder has, so a
  version a session is running is never touched. Only a version with its record
  counts as installed; ones installed before this are fetched again once. The
  files, the record and (on Unix) the folders are flushed to disk before the
  version is named current, and once Luna has installed this way a version
  without its record is never run. Sweeping old versions deletes nothing
  through a link or junction below them. Codex's package is unpacked with caps
  on its entries, its bytes as the tar reader writes them, each metadata record
  and the decompressed stream, and a link entry or PAX size override in it is
  refused.
- Shut down and restart when done turn the PC off again. Luna asks Windows
  itself, with the shutdown privilege turned on, instead of running
  `shutdown.exe`, which had been failing without a word; a refusal is logged
  with its Windows error.
- Windows is asked before any session is closed, so a refusal leaves every chat
  running, and a shutdown Windows records as canceled after the chats were
  closed brings them back where they were instead of leaving dead panes. One
  whose fate Windows does not tell leaves the chats closed and says how to bring
  them back, rather than reopening them under a shutdown still on its way.
- Windows' notice during the grace says that open programs are closed by force
  and unsaved work in them is lost, and so does the user guide.
- Shutdown winds Luna down the way restart does, so a chat parked mid-turn is
  not saved as finished.
- A restart that Windows refuses, or that is canceled, no longer leaves Luna set
  to start at the next sign-in.
- The release workflow failed at attesting the npm SBOM (`Unsupported SBOM
  format`): it was written without a serial number, which the attest action
  requires of a CycloneDX file. Both SBOMs now carry one and are checked for it
  as soon as they are made, and the attestations use `actions/attest` in place
  of the deprecated `actions/attest-sbom`.
- The release and test builds failed: `tauri build` refuses a Tauri crate and its
  npm package on different minor versions, and the notification plugin's crate
  had moved to 2.8 while `@tauri-apps/plugin-notification` stayed at 2.7. The
  package is on 2.8 now, and the gate checks every such pair in the lockfiles
  (`pnpm check:tauri`), so a bump to one side alone fails there.

## [0.10.0] - 2026-11-08

A security and engineering release. Agent tools now follow least privilege,
Luna's files are private from the moment they are created, and the build, tests
and releases run in CI. Everyday behavior is otherwise unchanged, apart from the
fixes below.

### Security

- A helper session can never hold more privilege than the session that started
  it, and only a session's own helpers accept its approvals and raw keystrokes.
- Text that a CLI would run as a command (a line starting with `/` or `!`)
  reaches a helper, visible or in the background, only when a full-access
  session sends it to its own helper.
- The app window runs under a strict Content Security Policy instead of none.
- Luna's data folder is made private when Luna is installed and checked again at
  every start, and files holding secrets are private from the moment they are
  created.
- A file in the data folder that someone else planted or replaced with a link
  is moved aside instead of being written to.
- On Windows, the install folder and the folders holding secrets get protected
  permissions of their own, and the installer resets only the permissions it
  owns.
- Secrets come from the operating system's secure random generator and are
  compared in constant time, and each chat's hook URLs carry a secret of their
  own.
- Claude Code receives its token for Luna's tools through the environment
  instead of a config file.
- Luna's local listeners cap connections before authentication, parse requests
  strictly and enforce time limits.
- A worktree path containing a line break can no longer forge a worktree
  record, and a Codex worktree is removed only from a Codex account's worktrees
  folder.

### Added

- A threat model and a vulnerability reporting policy (`SECURITY.md`).
- Documentation in `docs/`: a user guide by topic, architecture pages, decision
  records, notes on how each CLI behaves, and a development guide.
- A contributing guide, issue and pull request templates, and this changelog.

### Changed

- Luna is dual-licensed under MIT OR Apache-2.0.
- Releases are built, signed and attested by GitHub Actions from a version tag,
  behind a maintainer's approval, with SBOMs and checksums published beside the
  installer; `docs/releasing.md` explains how to verify a download.
- CI runs on every push on Linux and Windows: formatting, lints, type checks,
  the frontend's tests and build, and the Rust unit tests on both systems.
- Toolchains and build images are pinned to exact versions, and every build
  step's result is checked.
- The Rust code and the frontend are split into smaller modules along their
  natural seams, with no change in behavior.
- Test coverage grows: frontend tests, golden tests of how both CLIs are
  started, and a live suite against the real CLIs on throwaway accounts.
- The repository is written in US English, which a check enforces, and commit
  messages carry no AI attribution, which a hook and CI enforce.

### Fixed

- Worktrees in a folder written with 8.3 short names, such as a temp or profile
  folder under a long user name, are recognized, so helpers return to their
  worktree and Codex chats keep their history.
- Running Luna as the built-in Windows Administrator no longer finds the data
  folder open to others on every start.
- An elevated Luna no longer moves aside the logs and lock files it created
  itself.
- Enter in the account picker acts on the highlighted account, even after the
  list is reordered or that account is removed.
- A line break typed into an answer or note no longer submits it early and sends
  the rest as a separate prompt.
- Text sent to a background helper after its turn was stopped now starts the
  next turn instead of waiting indefinitely.
- A background helper whose queued messages were all refused no longer starts
  an empty turn, and the sender is told that its text was refused.

### Removed

- `ARCHITECTURE.md`, replaced by the architecture pages in `docs/`.
- The Russian README; the repository's text is in English only.

## [0.9.0] - 2026-11-04

- A folder's skills reach the sessions Luna starts in it, worktrees included.
- A turn that ends on a question to the user shows as waiting.
- 0.9.1–0.9.10 (to 2026-11-06): fixes and refinements.

## [0.8.0] - 2026-11-01

- Helpers survive a core restart, notice loops are broken, read-only Codex with
  network (`read-only-net`), helpers watched without reading them.
- 0.8.2–0.8.3: what a turn leaves running is named and no longer keeps the PC up
  for good; a Linux check.

## [0.7.0] - 2026-10-29

- Skills and instructions of your own, laid out into accounts and folders.
- 0.7.3: background helpers outlive the core; links open in the browser.

## [0.6.0] - 2026-10-15

- The Windows XP look, toned for reading all day; the sidebar slides out from the
  window's edge; Alt+digit peeks a pane.
- 0.6.11: sessions run in a separate core process.
- 0.6.23: background helpers; their questions reach the agent; Codex's sandbox
  works.

## [0.5.0] - 2026-10-15

- The shell redesigned: a task-pane sidebar, a full-width status bar, one icon set.

## [0.4.0] - 2026-09-24

- Agents: a session can spawn, drive and delete other sessions over MCP.
- Keep the PC awake while sessions work, and power off once they are done.

## [0.3.0] - 2026-09-17

- Codex as a second provider.

## [0.2.0] - 2026-08-14

- The installer is released with each version.
- 0.2.16: Luna ships, verifies and updates its own Claude Code binary.

## [0.1.0] - 2026-08-14

- First release: Claude Code sessions in panes, accounts with isolated configs,
  usage limits, worktrees, the updater.

[Unreleased]: https://github.com/nrubinmosin/luna/compare/v0.19.0...HEAD
[0.19.0]: https://github.com/nrubinmosin/luna/compare/v0.18.5...v0.19.0
[0.18.5]: https://github.com/nrubinmosin/luna/compare/v0.18.4...v0.18.5
[0.18.4]: https://github.com/nrubinmosin/luna/compare/v0.18.2...v0.18.4
[0.18.2]: https://github.com/nrubinmosin/luna/compare/v0.18.1...v0.18.2
[0.18.1]: https://github.com/nrubinmosin/luna/compare/v0.18.0...v0.18.1
[0.18.0]: https://github.com/nrubinmosin/luna/compare/v0.17.0...v0.18.0
[0.17.0]: https://github.com/nrubinmosin/luna/compare/v0.16.0...v0.17.0
[0.16.0]: https://github.com/nrubinmosin/luna/compare/v0.15.0...v0.16.0
[0.15.0]: https://github.com/nrubinmosin/luna/compare/v0.14.0...v0.15.0
[0.14.0]: https://github.com/nrubinmosin/luna/compare/v0.13.0...v0.14.0
[0.13.0]: https://github.com/nrubinmosin/luna/compare/v0.12.0...v0.13.0
[0.12.0]: https://github.com/nrubinmosin/luna/compare/v0.11.0...v0.12.0
[0.11.0]: https://github.com/nrubinmosin/luna/compare/v0.10.2...v0.11.0
[0.10.2]: https://github.com/nrubinmosin/luna/compare/v0.10.1...v0.10.2
[0.10.1]: https://github.com/nrubinmosin/luna/compare/v0.10.0...v0.10.1
[0.10.0]: https://github.com/nrubinmosin/luna/compare/v0.9.10...v0.10.0
[0.9.0]: https://github.com/nrubinmosin/luna/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/nrubinmosin/luna/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/nrubinmosin/luna/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/nrubinmosin/luna/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/nrubinmosin/luna/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/nrubinmosin/luna/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/nrubinmosin/luna/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/nrubinmosin/luna/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/nrubinmosin/luna/releases/tag/v0.1.0
