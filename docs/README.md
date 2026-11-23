# Luna documentation

## User guide

- [The window and panes](user-guide/window.md): layouts, peek, boards, status
  bar, shortcuts, themes
- [Chats](user-guide/chats.md): the new-chat dialog for each CLI, status, titles,
  clipboard and terminal input
- [Accounts](user-guide/accounts.md): account folders, live limits, trust,
  what Luna passes at every start, shared settings
- [Skills and instructions](user-guide/library.md): the library, reaches,
  folder overrides, managed copies
- [Sessions and cleanup](user-guide/sessions.md): the core, quitting, restarts,
  orphans, the log
- [Agents and MCP tools](user-guide/agents.md): helpers, talking to sessions,
  answers, notices, lifetime
- [What Luna adds to a model's context](user-guide/context.md): every text Luna
  puts before a model, by channel and role, its size and how to turn it off
- [Power and shutdown-when-done](user-guide/power.md)
- [Updates](user-guide/updates.md): Luna and the CLIs
- [Settings](user-guide/settings.md): what each page holds and where it went

## Architecture

- [Overview](architecture/overview.md): processes, IPC, data flow, module map
- [Agents](architecture/agents.md): MCP tools, registry, debts, notifier, rights
- [Background helpers](architecture/background-helpers.md): headless turns, and
  what outlives a core
- [pty and launch](architecture/pty-and-launch.md): command lines, hooks,
  golden tests, terminal quirks
- [Providers](architecture/providers.md): `claude/` and `codex/`, reading
  sessions, the Codex sandbox
- [Accounts and account files](architecture/accounts.md): limits, OAuth, trust,
  shared settings, safe writes
- [Library](architecture/library.md)
- [Power and lifecycle](architecture/power-and-lifecycle.md): busyness, jobs,
  keep-awake, core and window lifecycle, updates
- [Desktop services](architecture/desktop-services.md): the tray,
  notifications, single instance, pickers, clipboard, window state, updater,
  installer and macOS bundle
- [The native window](architecture/native-window.md): the GPUI window crate, its
  host, models, theme and font, and the screenshot harness
- [CLI compatibility notes](cli-compatibility.md): observed Claude Code and Codex
  behavior by version

## Decisions (ADRs)

| ADR | Decision | Status |
| --- | --- | --- |
| [0001](adr/0001-session-state-lives-in-the-cli.md) | Session state lives in the CLI; the Rust core stays thin | Superseded in part by 0030 |
| [0002](adr/0002-core-process-outlives-the-window.md) | Sessions run in a core process that outlives the window | Superseded in part by 0030 |
| [0003](adr/0003-loopback-hub-for-hooks-and-mcp.md) | One loopback HTTP hub for hooks and MCP | Accepted |
| [0004](adr/0004-docker-only-toolchain.md) | Everything builds in Docker on pinned toolchains | Superseded in part by 0030 |
| [0005](adr/0005-two-providers-as-independent-layers.md) | Claude Code and Codex are two independent layers | Superseded in part by 0030 |
| [0006](adr/0006-isolated-test-flavor.md) | A test flavor isolated from the Luna in use | Superseded in part by 0030 |
| [0007](adr/0007-least-privilege-for-agent-tools.md) | A least-privilege contract for agent tools | Superseded by 0030's amendment |
| [0008](adr/0008-strict-csp-with-inline-styles.md) | A strict CSP, with inline styles allowed | Superseded by 0030 |
| [0009](adr/0009-modules-now-crates-later.md) | Modules now, crates later | Accepted |
| [0010](adr/0010-registry-encapsulation.md) | Encapsulate the agents Registry behind transition methods | Proposed |
| [0011](adr/0011-private-data-dir-beside-the-exe.md) | Luna's data lives in one private folder beside the exe | Superseded in part by 0030 |
| [0012](adr/0012-agents-over-mcp-with-notices.md) | Agents drive sessions over MCP; answers come back as notices | Accepted, reach narrowed by 0025 |
| [0013](adr/0013-own-cli-binaries.md) | Luna ships and updates its own copies of the CLIs | Accepted |
| [0014](adr/0014-core-credential-behind-a-read-deny.md) | The core credential stays a bearer token behind a read deny | Superseded by 0021 |
| [0015](adr/0015-the-page-gets-only-what-it-calls.md) | The page gets only the commands it calls | Superseded by 0030 |
| [0016](adr/0016-luna-is-a-runtime.md) | Luna is a runtime, not a provider configurator | Accepted |
| [0017](adr/0017-helpers-run-in-the-background-only.md) | Helpers run in the background only; opening one hands it to the person | Accepted |
| [0018](adr/0018-mcp-tools-by-resource-and-verb.md) | Name the MCP tools by resource and verb, and break the old names | Accepted |
| [0019](adr/0019-cli-channels-and-rollback.md) | CLI channels and rollback: a known-good list per release, an offline probe, a canary that asks | Superseded in part by 0030 |
| [0020](adr/0020-session-self-handoff.md) | Hand a chat over to a fresh conversation in place, on its own word | Superseded by 0030 |
| [0021](adr/0021-core-endpoint-by-os-identity.md) | The core lets in whom the OS vouches for, and keeps no credential | Accepted |
| [0022](adr/0022-content-luna-serves-at-its-own-trigger.md) | Serve the handoff's instructions through Luna's own tool, to the chat that opted in alone | Superseded by 0030 |
| [0023](adr/0023-moving-a-chat-to-another-account.md) | Move a chat to another account of its CLI by a document the person confirms | Superseded by 0030 |
| [0024](adr/0024-handoff-on-request.md) | List the handoff to every chat with Luna's tools, and answer it only when asked | Superseded by 0030 |
| [0025](adr/0025-a-helper-reaches-only-its-own-subtree.md) | A helper reaches only its own subtree | Accepted |
| [0026](adr/0026-session-roles-at-memory-level.md) | Give sessions a role, chat, lead or helper, each instructed at memory level | Accepted, amended by 0030 |
| [0027](adr/0027-auto-handoff-for-leads.md) | Hand a lead off to a fresh conversation of its own at its turn's end | Superseded by 0030 |
| [0028](adr/0028-writes-into-a-helper-come-from-above.md) | Only the sessions above a helper write into it; the person moves it | Accepted, amended by 0030 |
| [0029](adr/0029-dependencies-move-by-hand.md) | Move every pin in the project's own commits; Dependabot only alerts | Superseded in part by 0030 |
| [0030](adr/0030-scope-of-0-20.md) | Release 0.20 is a smaller Luna in a native GPUI window | Accepted |
| [0031](adr/0031-one-lifecycle-record-per-session.md) | Keep one lifecycle record per session, from the CLIs' own signals | Accepted |
| [0032](adr/0032-codex-helpers-on-app-server.md) | Run each Codex helper turn as one `codex app-server`, steered and stopped through it | Accepted |
| [0034](adr/0034-luna-default-permission-mode.md) | Luna supplies a permission mode when the CLI config leaves it unset | Accepted |

New decisions start from the [template](adr/0000-template.md).

## Working on Luna

- [Development](development.md): toolchains, scripts, the gate, the test build,
  CI, adding a feature end to end
- [Testing](testing.md)
- [Releasing](releasing.md)
- [Contributing](../CONTRIBUTING.md) and [changelog](../CHANGELOG.md)
- [Security policy](../SECURITY.md) and [threat model](security/threat-model.md)
- [Brand assets](brand/README.md)
