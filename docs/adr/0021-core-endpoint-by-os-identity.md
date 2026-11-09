# ADR-0021: The core lets in whom the OS vouches for, and keeps no credential

- **Status:** Accepted
- **Date:** 2026-11-09
- Supersedes [ADR-0014](0014-core-credential-behind-a-read-deny.md)

## Context

Through Luna 0.11 a window reached the core on a loopback TCP port, and the
first frame had to carry the 256-bit token the core wrote to
`<data>/core/core.json` (ADR-0014). That token was the whole of the core's
authority, sitting in a file every process of the user can read, CLI sandboxes
included. ADR-0014 kept it behind a read deny where the CLI could enforce one,
and on Windows behind a check of the process that owned the connecting socket.

Three things changed the weighing:

- **A deputy defeats a per-socket check.** Measured against 0.11.0 on Windows
  with Codex 0.167.0: a client run in `codex sandbox` (`read-only` and
  `workspace-write`, unelevated) read the token from `core.json`; its own
  connection to the core was refused by the peer check, but the same client
  through a plain loopback TCP forwarder (any HTTP or SOCKS proxy a person runs
  will do) got its hello welcomed and `info` answered: full authority. The
  check judges whoever owns the socket, and the forwarder is a plain process.
- **The Linux premise was stale.** ADR-0014 held Codex's Linux sandbox to be
  Landlock, in the core's own namespaces, so a sandboxed peer could not be told
  apart. Codex 0.167.0 runs its commands under bubblewrap, in a user namespace
  of their own, with `no_new_privs` set for its seccomp filter; legacy Landlock
  is opt-in and refused for filesystem-restricted policies. A Linux peer can be
  told apart.
- **A token on disk keeps every reader a holder.** Claude Code's Bash, where it
  runs unasked, and any other same-user process could act as the window.

A council of two reviews (2026-11-09) agreed to remove the disk credential in
the next release and split on how: core-created windows with inherited channels
(protocol v2 in ADR-0014's sense), or no secret at all, with the OS's word on
the connecting process as the key, on an endpoint only this user can open. The
owner chose the second: Luna is released for Windows only; it closes the
generic loopback forwarder outright (the endpoint is no TCP port), keeps the
window and core lifecycle as it is, and adds no new class of leak (a channel
handle passed to a CLI). The concerns of the other review are kept as tests.

## Decision

No secret exists. Authority is what R3 already states: an unconfined process
of this user, as the OS attests at connect.

- **Windows** (`endpoint/win.rs`): a named pipe `\\.\pipe\luna-core-<128
  random bits>`, a fresh name per core, made with
  `FILE_FLAG_FIRST_PIPE_INSTANCE` (a name someone else holds is refused, and
  every later instance must read back the first one's security, so none joins
  a pipe of someone else's) and `PIPE_REJECT_REMOTE_CLIENTS`; a protected DACL
  of the core's user and SYSTEM (full) and Administrators (read and write), and
  a medium mandatory label with no-read-up and no-write-up. Every handle is
  overlapped and not inheritable; reads and writes wait with their own
  deadlines. The core impersonates the client (`ImpersonateNamedPipeClient`)
  just long enough to open the token the pipe was opened with, and judges that
  token (`peer/win.rs`): its user, whether it is restricted or an
  AppContainer, its integrity. The window opens the pipe at identification
  level, with a static security context, so a server that is not the core can
  act as nobody; a client that opens without one (a shell's redirect) cannot be
  impersonated before it writes, and is refused.
- **Linux and macOS** (`endpoint/unix.rs`): a Unix socket in the core folder,
  which is `0700`. The core reads the kernel's record of the client
  (`SO_PEERCRED`; macOS `getpeereid` and `LOCAL_PEERPID`) and on Linux, from
  `/proc/<pid>`, its user namespace, `no_new_privs` and seccomp state (mode
  and filter count); the facts count only if the pidfd the kernel gives for
  the peer (`SO_PEERPIDFD`, 6.5) shows that process still there after they
  were read. Any failure to get one refuses, except on a kernel that does not
  know the option (`ENOPROTOOPT`), where the pid alone is checked.
- **The decision** (`peer/mod.rs` `verdict`), pure and fail closed: refused when
  the facts cannot be read, the token is restricted or an AppContainer, the
  process is in another user namespace, `no_new_privs` is set where the core's
  is not, or its seccomp state is stricter than the core's (a higher mode, or
  more filters: they are only ever added, and a child starts with its
  parent's); another user is refused, except an elevated administrator (or
  root), who may only stop it — through 0.11 Administrators could read
  `core.json` and so do anything; below the core's integrity is refused, except
  that a core run as administrator, which does no work, lets an ordinary window
  stop it and nothing else (F11).
- **Discovery** (`ipc.rs` `Discovery`): `core.json` holds the core's pid, its
  creation time, the protocol (2) and the endpoint's name. The window checks
  that the endpoint's server is that pid (`GetNamedPipeServerProcessId`, the
  socket's peer credentials) before it says anything (`corelink.rs` `hello`).
- **Migration** (`corelink/v1.rs`): a `core.json` with a token is a protocol-1
  core's. The window uses the token for the hello, to learn its session count,
  and for a shutdown, nothing else: an idle one is replaced at once, one with
  sessions shows the mismatch banner until the person restarts it, as any core
  of another build. `luna --stop-core` goes through the endpoint like any
  window, so it is no shutdown for anyone who asks; the installer's hook is
  unchanged.
- Every limit of the TCP listener holds on the endpoint: the stranger slots,
  the hello's deadline and size, frame sizes, the per-window queue, the
  command lanes, the elevated core's `info`/`shutdown`-only rule.

The read denies of ADR-0014 stay as a second wall: Claude Code's file tools and
Codex's read-only profiles off Windows are kept out of `core/`, where the
socket is off Windows; nothing in it lets anyone in any more.

## Consequences

- Covered by tests that fail without their part: the verdict's cases
  (`peer/mod.rs`); a restricted token that the DACL lets through refused by
  its token, a low-integrity client refused the open, an anonymous client
  refused (`peer/win.rs`); a client with `no_new_privs`, one under a stricter
  seccomp state and one in a user namespace of its own refused
  (`peer/unix.rs`, the last where user namespaces are allowed); a squatted name refused (`endpoint/win.rs`,
  `endpoint/unix.rs`); an endpoint served by a process other than the one
  `core.json` names hears nothing (`corelink.rs`); discovery serializes no
  credential (`ipc.rs`); a protocol-1 core hears a hello and a shutdown and
  nothing else (`corelink.rs`); the hello's deadline and caps over the new
  transport (`coreproc.rs`, `admit.rs`). Guards against regression: no
  endpoint handle is inheritable (`endpoint/mod.rs`), no session start carries
  anything of the core's (`pty/start.rs`), the core's command line holds no
  secret (`corelink.rs`). Live (`live.rs`): Codex's Windows sandbox refused the
  pipe's open in both modes, a plain process welcomed, a low-integrity one
  refused; Codex's Linux sandbox refused at the socket.
- Residuals: any unconfined process of the user's is the window's equal, as
  before (R3): Claude Code's Bash among them. A deputy that opens the pipe or
  the socket for a confined client: a plain process relaying between a
  loopback port and the endpoint is a program written to do so, no longer a
  proxy that happens to run. A confinement the OS does not mark on the token or
  the namespace: macOS Seatbelt is not seen at the socket, and Claude Code's
  own sandbox is not run by Luna. A core started with `no_new_privs`
  (Flatpak, systemd's NoNewPrivileges, a container) cannot refuse for that
  bit, which every window it serves carries too, and cannot tell a client
  confined by Landlock alone, in the core's own user namespace and under its
  seccomp state, from itself: the kernel exposes no Landlock domain to procfs
  or to any other process (no status field, no LSM attribute, no query in
  Landlock's ABI; checked against the kernel's documentation and source and on
  6.18, where a Landlock-restricted process's status differs from a plain one's
  only in `NoNewPrivs`). Such a core still refuses another namespace or an
  added seccomp filter, and says once at its start that it runs so. On a
  kernel before 6.5, without `SO_PEERPIDFD`, a confined client that leaves its
  connection to another process and exits could, should an unconfined process
  take its pid before the core reads `/proc`, be read as that process. A
  protocol-1 core until it is replaced. A
  Luna older than 0.12 meeting a 0.12 core (a downgrade) does not find it and
  shows the core down until the newer core is stopped (tray Quit or its
  `--stop-core`).
- The next step, if a deputy or an unmarked sandbox is found able to reach the
  endpoint: core-created windows with inherited channels, which would reuse this
  endpoint's admission for their rendezvous.
