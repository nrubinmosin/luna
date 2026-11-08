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

- Each CLI's release channel is checked every six hours; a new version installs
  into `<data>/claude-cli/versions/<ver>/` or `<data>/codex-cli/versions/<ver>/`
  beside the old one, which goes once no session uses it. A download is
  checked whole before it counts, so one cut short (Luna closed, the PC off)
  is simply fetched again on the next check.
- Claude Code comes as a bare exe from the official release bucket, verified
  against its manifest's sha256, and the manifest against Anthropic's release
  signing key, which Luna carries. Codex comes from GitHub Releases
  (`openai/codex`, tag `rust-v<ver>`) as the package tarball, verified against
  the release's checksums, with the helpers its Windows sandbox needs.
- Claude Code sessions run with `DISABLE_AUTOUPDATER=1` and Codex accounts are
  created with `check_for_update_on_startup = false`, so Luna is the only thing
  that moves either binary.

## Where to see versions

All three versions live in Settings (the gear in the status bar), each with its
"checked … ago" and a check-now button. The status bar speaks up only while there
is news: an update on offer, a download running, or a failure worth a retry.
