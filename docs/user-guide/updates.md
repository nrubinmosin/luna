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
  end it. From the moment the stop begins until the updated Luna takes over,
  nothing new starts: a chat opened then says "Luna is updating", text sent
  to a helper waits (the updated Luna tells its sender it was not
  delivered), and Luna types no notice or message into a chat (they wait
  too); what you type yourself still goes in. However long the installer
  waits on Windows' administrator prompt, that holds. If the install fails,
  everything can start again at once; if it never happens (the prompt
  declined, the PC restarted first), it can once you open the same Luna
  again. On Linux and macOS nothing starts until you restart Luna on the new
  version. Only
  a session that would not stop in time has the updated Luna ask before it
  restarts the core. Updating from 0.19, the old
  version does the stopping, and it may ask once when a helper left something
  running.
- The updater installs into the folder Luna was installed in, where it keeps its
  data. From 0.20 its installer never puts an older Luna over a newer one
  there: a copy of Luna run from another folder may be offered a release older
  than the one installed, and that update stops with a message instead. A
  release candidate counts as older than its release, and an earlier
  candidate (rc.1) as older than a later one (rc.2). A setup that cannot
  write all of Luna's files (another program holds one), or Windows' record
  of the install, stops with a message saying so and does not start Luna
  again: close what holds the files and run it again.
- The update's setup is downloaded into the `updates` folder in Luna's data
  folder, with a small record beside it that Luna put it there. Luna
  deletes the setups earlier updates left there, and their records, as it
  starts, once they are an hour old; anything else in that folder stays,
  a setup you saved there yourself included.

## Switching from 0.20

Do not install 0.21 with 0.20's update chip: 0.20 stops your sessions
before it runs the setup, and the setup then refuses. Switch by hand, as
below.

0.21 installs under new names: the exe `rm-luna.exe`, the registry keys
`HKCU\Software\rm.luna` and the uninstall entry `rm.luna`, and the default
folder `%LOCALAPPDATA%\rm-luna`. It does not update 0.20 in place, and its
setup refuses to install while 0.20 is installed. If you did click the
chip, the setup stops with a message and changes nothing; start 0.20 again
from the Start menu to get your sessions back. To switch, with `$old` the
folder 0.20 is installed in (Settings, Apps, Installed apps, Luna shows it),
`$data` the folder 0.20 kept its data in, and `$backup` a folder of your
choice that only you can open (in your user folder, not on a shared
drive):

`$data` is `$old` itself, unless 0.20 could not write there; then it is
`%LOCALAPPDATA%\luna`. It is the folder that holds `luna.log`.

1. Quit Luna (the tray's **Quit**), which ends its chats and helpers. Nothing
   of it should still run: `Get-Process | Where-Object { $_.Path -like
   "$old\*" -or $_.Path -like "$data\*" }` shows nothing.
2. Back up its data files, their permissions with them (`/SEC`: the chats
   and the tokens in `hooks` and `mcp` stay yours alone rather than taking
   the backup folder's):
   `robocopy $data $backup /E /SEC /XD claude-cli codex-cli updates core webview /XF luna.exe uninstall.exe THIRD-PARTY-NOTICES.txt`.
   Delete the backup once 0.21 runs with your data.
3. Uninstall 0.20 (Installed apps, Luna, **Uninstall**), leaving "Delete
   the application data" unticked. It removes `luna.exe`, `uninstall.exe`,
   `THIRD-PARTY-NOTICES.txt`, the `core` folder, the Start-menu and desktop
   shortcuts and its uninstall entry, and leaves every data file in `$data`.
   A pinned taskbar item goes with the shortcut; pin 0.21 again later.
   `HKCU\Software\luna\Luna` (where 0.20 was installed) stays; it is no
   longer read, and you may delete it.
4. Move the old folder aside, for example `Rename-Item $old Luna-0.20`: the
   0.21 setup refuses a folder that holds files of another install.
5. Install 0.21 with its setup, `rm-luna_<version>_x64-setup.exe`. To keep
   Luna where it was, choose `$old` as the folder (the setup proposes
   `%LOCALAPPDATA%\rm-luna`). On the last page untick **Run Luna**, or quit
   it from the tray before the next step.
6. Copy the data files back from where they are now (`$moved`, the folder
   step 4 made, or `$data` where that was `%LOCALAPPDATA%\luna`) into 0.21's
   data folder (its install folder, or `%LOCALAPPDATA%\rm-luna` where 0.21
   cannot write there):
   `robocopy $moved <new folder> /E /SEC /XD core updates webview /XF luna.exe uninstall.exe THIRD-PARTY-NOTICES.txt luna.run luna.core.run *.lock update-stop.json nvidia-profile.json nvidia-setting.json`.
   This carries your settings, window state, chats and helpers, the library
   and its record, the logs and the downloaded CLIs. Left out: the running
   markers, the locks and the `core` folder, which name the old exe or the
   core's old pipe, and the two NVIDIA records, whose profile and setting
   0.20's uninstall already undid (a stale one could make 0.21 report its
   profile gone instead of making one).
7. Start Luna from the Start menu. Its log (`luna.log`) shows
   `starting luna 0.21`; where an NVIDIA driver is present and Luna made
   its profile, also `G-SYNC turned off for Luna in its own NVIDIA profile`.
8. Once the library has run its first pass (at the start, or **Sync** in the
   library), look in `plugins\cache\luna` in each Codex account folder.
   0.21 lays the library's folder skills out under `plugins\cache\rm-luna`
   and takes the skill copies it had put under `luna` out; what it leaves
   there, the manifests under `.codex-plugin` and the empty folders, is no
   longer used: delete those, and keep anything you put there yourself.
9. When all is well, delete the old folder and the backup.

## The CLIs

Luna carries its own copies of `claude` and `codex` and keeps them current, so
nothing depends on a global install.

- Each CLI is kept at its newest release. Luna asks the CLI's own release
  channel as its window starts and every six hours while the window runs (or
  when you click **Check now** in Settings → CLIs); a newer version installs into `<data>/claude-cli/versions/<ver>/` or
  `<data>/codex-cli/versions/<ver>/` beside the old one. A download is checked
  whole before it counts, so one cut short (Luna closed, the PC off) is simply
  fetched again on the next check.
- On a fresh install Luna downloads both CLIs as it first starts. A chat
  opened before its CLI is in says it is waiting for the download, with how
  far it has got, and starts by itself once the CLI is there. If the download
  fails, the chat says why and offers **Retry**. A helper spawned meanwhile
  waits for the download too, ten minutes at most, and is refused with the
  reason if it fails.
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
  the release's checksums, with the helpers its Windows sandbox needs. On
  Windows both are also held to their publisher's signature: a download whose
  program files (`.exe` and `.dll`) are not signed by "Anthropic, PBC" (Claude
  Code) or "OpenAI OpCo, LLC" (Codex; a runtime DLL it bundles may carry
  Microsoft's, and its bundled `rg.exe` ships unsigned and is checked only
  against the release's checksum) is refused and never becomes current.
- Claude Code sessions run with `DISABLE_AUTOUPDATER=1` and Codex accounts are
  created with `check_for_update_on_startup = false`, so Luna is the only thing
  that moves either binary.

## Where to see versions

Luna's own version and the core's live in Settings → Updates (the gear in the status bar),
each with its "checked … ago" and a button; the CLIs' in Settings → CLIs, with their
installed versions and the ones Luna turned away, with why. The status bar speaks up only while there is news: an
update on offer, a download running, a failure worth a retry, or a new CLI version whose
first sessions failed with nothing to go back to.
