# Updates

## Luna

- On startup Luna checks `latest.json` from GitHub Releases. A release found there
  lights a chip in the status bar and waits to be clicked, rather than putting up
  a modal over whatever is mid-turn. Nothing installs without that click.
- A release whose Rust side is the same as the running core's restarts only the
  window: sessions keep running in the core and the new window picks them up.
- Any other release stops them: the dialog lists them, and once the installer is
  downloaded every one is asked to quit, each with its exit hooks; the chats
  resume from their transcripts afterwards. Background helpers' turns are
  stopped too, and on Windows what their turns left running (a shell, a
  watcher, a build) is ended, so the updated Luna replaces the old core at
  once. On Linux and macOS Luna does not see what a helper's turns left: it
  does not hold up the update, and keeps running after it until it ends or you
  end it. Only a session that started after the stop (a helper sent text
  meanwhile, say), or one that would not stop in time, has the updated Luna
  ask before it restarts the core. Updating from 0.19, the old
  version does the stopping, and it may ask once when a helper left something
  running.
- The updater installs into the folder Luna was installed in, where it keeps its
  data. From 0.20 its installer never puts an older Luna over a newer one
  there: a copy of Luna run from another folder may be offered a release older
  than the one installed, and that update stops with a message instead. A
  release candidate counts as older than its release, and an earlier
  candidate (rc.1) as older than a later one (rc.2).

## The CLIs

Luna carries its own copies of `claude` and `codex` and keeps them current, so
nothing depends on a global install.

- Each CLI is kept at its newest release. Luna asks the CLI's own release
  channel as its window starts and every six hours while the window runs (or
  when you click **Check now** in Settings → CLIs); a newer version installs into `<data>/claude-cli/versions/<ver>/` or
  `<data>/codex-cli/versions/<ver>/` beside the old one. A download is checked
  whole before it counts, so one cut short (Luna closed, the PC off) is simply
  fetched again on the next check.
- Each Luna release names the oldest version of each CLI it runs. Nothing
  older is downloaded, used or gone back to, and Luna never steps down from
  the version it runs, even if the release channel goes back to an older one.
  Only a Luna update raises that oldest version, when an older one is found
  unsafe. When it rises past the CLI you have, no chat or helper starts on it
  (the start says why) until the next check installs a newer one, which it
  does at once.
- Before a new version is used, Luna runs it once offline to see that it
  still knows every flag Luna passes it (and, for Codex, takes the hooks Luna
  gives it). One that answers wrong is not used, and not tried again while it
  is the newest release. One that could not be run to its end (a virus scanner
  still holding the fresh download, say) is simply asked again at the next
  check.
- The version before is always kept, and so is any version a session still
  runs on; older ones go once nothing runs them. Running sessions keep the
  version they started with; new ones start on the current one.
- The first three sessions on a new version are watched. If two of them fail
  soon after starting, Luna goes back to the version before on its own, says
  so in a desktop notice, and does not take the failed version again while it
  is the newest release; a newer one installs as usual. With no
  version before to go back to (or one Luna cannot vouch for: older than the
  oldest it runs, or its files no longer matching their checksums), the
  notice says so and the version stays. If Luna is closed after the failures
  were counted but before it went back, it goes back as it next starts. A
  session you or Luna stopped does not count, nor one that never started, nor
  one that ended on something that was not the CLI's fault: a worktree asked
  for outside a git repository, a sign-in missing or expired, a usage limit,
  or the network or the provider's service failing, each known by the error
  the CLI ended on (for a chat, the last line it printed); the next session
  is watched in its place. If the version gone back to fails its first
  sessions too, Luna stays on it: the failures were then not the version's.
- **Roll back**, a control of the new window's Settings → CLIs (it arrives
  with that window; the old window has none), does the same on your word: it
  goes back to the version before (shown beside the button) and does not take
  the version it left again while that is the newest release; the next
  different release installs as usual. It checks what the automatic rollback
  checks and refuses, saying why, when nothing is kept to go back to or that
  version is older than the oldest this Luna runs, was turned away, or has
  files that no longer match their checksums, or when the version it was to
  leave is not the one now current (a check installed a newer one since you
  looked). Running sessions keep their version.
- What Luna keeps of this (which version is current, the one before, the
  versions it turned away) is one file in each CLI's folder. If it is lost or
  damaged, Luna takes up again the newest installed version whose files still
  match their checksums; which versions it had turned away is lost with it,
  so such a version may be tried once more.
- Luna 0.19's **Known-good** and **Latest** channels, **Use**, **Mark bad**,
  **Mark good** and the rest are gone from Luna; until Luna's new window
  comes, the old window still shows some of them in Settings → CLIs, and they
  only fail. What they left on disk is read in and removed at the first
  check; a version marked bad then stays skipped while it is the newest
  release, and the watch over the first sessions of the version you had is not
  carried over, so nothing rolls back from it.
- While Luna runs as administrator it does not update the CLIs: the check
  before a new version is used would run it with administrator rights.
- Claude Code comes as a bare exe from the official release bucket, verified
  against its manifest's sha256, and the manifest against Anthropic's release
  signing key, which Luna carries. Codex comes from GitHub Releases
  (`openai/codex`, tag `rust-v<ver>`) as the package tarball, verified against
  the release's checksums, with the helpers its Windows sandbox needs.
- Claude Code sessions run with `DISABLE_AUTOUPDATER=1` and Codex accounts are
  created with `check_for_update_on_startup = false`, so Luna is the only thing
  that moves either binary.

## Where to see versions

Luna's own version and the core's live in Settings → Updates (the gear in the status bar),
each with its "checked … ago" and a button; the CLIs' in Settings → CLIs, with their
installed versions and the ones Luna turned away, with why. The status bar speaks up only while there is news: an
update on offer, a download running, a failure worth a retry, or a new CLI version whose
first sessions failed with nothing to go back to.
