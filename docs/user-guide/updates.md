# Updates

## Luna

- On startup Luna checks `latest.json` from GitHub Releases. A release found there
  lights a chip in the status bar and waits to be clicked, rather than putting up
  a modal over whatever is mid-turn. Nothing installs without that click.
- A release whose Rust side is the same as the running core's restarts only the
  window: sessions keep running in the core and the new window picks them up.
- Any other release stops them: the dialog lists them, and once the installer is
  downloaded every one is asked to quit, each with its exit hooks; the chats
  resume from their transcripts afterwards.
- The updater installs into the folder Luna was installed in, where it keeps its
  data.

## The CLIs

Luna carries its own copies of `claude` and `codex` and keeps them current, so
nothing depends on a global install.

- Each CLI follows a channel of its own, chosen in Settings → CLIs:
  **Known-good** (the default) installs the newest version this Luna release
  lists as checked with it; **Latest** installs each new release of the CLI.
  A CLI release with a security fix reaches the known-good channel as a Luna
  update.
- The channel is checked every six hours; a new version installs into
  `<data>/claude-cli/versions/<ver>/` or `<data>/codex-cli/versions/<ver>/`
  beside the old one. A download is checked whole before it counts, so one cut
  short (Luna closed, the PC off) is simply fetched again on the next check.
- Before a new version is used, Luna runs it once offline to see that it
  still knows every flag Luna passes it (and, for Codex, takes the hooks Luna
  gives it). One that does not is marked bad and never used.
- The version before is always kept, and so is any version a session still
  runs on; older ones go once nothing runs them. Running sessions keep the
  version they started with; new ones start on the current one.
- The first three sessions on a new version are watched. If two of them fail
  soon after starting, a notice says so and a chip in the status bar opens
  Settings → CLIs, where **Roll back** goes to the version before and **Keep
  it** keeps this one. Luna never rolls back on its own.
- **Use** on any installed version makes it the current one and keeps it
  there until you choose a channel again (**Follow channel**); **Mark bad**
  keeps a version from being installed or used again by an update. Going back
  checks the version's files against their checksums first, and nothing older
  than the oldest safe version a Luna release names ever runs.
- When a Luna update raises that oldest safe version past the CLI you have,
  no chat or helper starts on it (the start says why) until the next check
  installs a newer one, which it does at once, a version you held by hand
  included.
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
channels and installed versions. The status bar speaks up only while there is news: an
update on offer, a download running, a failure worth a retry, or a new CLI version whose
first sessions failed.
