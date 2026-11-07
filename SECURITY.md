# Security policy

## Supported versions

Only the latest release on
[GitHub Releases](https://github.com/nrubinmosin/luna/releases/latest) gets
security fixes. Luna updates itself, so a fix ships as a new release; older
versions are not patched.

## Reporting a vulnerability

Report privately through GitHub's
[Private Vulnerability Reporting](https://github.com/nrubinmosin/luna/security/advisories/new)
(the repository's **Security** tab, **Report a vulnerability**). Please do not
open a public issue, discussion or pull request for a suspected vulnerability.

A useful report says which version and OS, what an attacker needs (local
account, a session's MCP token, a malicious repository, a network position),
the steps or a proof of concept, and the impact you observed.

## Response targets

| Step | Target |
| --- | --- |
| Acknowledge the report | 3 business days |
| Initial assessment and severity | 10 business days |
| Fix released for critical or high severity | 30 days |
| Fix released for medium or low severity | next regular release, or 90 days |

You are kept informed through the advisory, credited in it unless you prefer
otherwise, and asked before details are published. Luna is maintained by one
person; these are targets, not guarantees.

## Scope

In scope: the Luna app built from this repository, including

- the core process and its loopback listeners (the window-to-core socket, the
  hook and MCP hub) and their authentication;
- authorization between agent sessions through Luna's MCP tools;
- the files Luna writes: its data folder, account folders it syncs into,
  worktrees it creates and removes;
- how Luna launches the CLIs (arguments, environment, hook commands);
- the updater and the release signing chain;
- the WebView UI and its Tauri commands.

Out of scope:

- vulnerabilities in Claude Code, Codex, the model APIs or other dependencies
  themselves (report them upstream; a way Luna makes one worse is in scope);
- what an agent does within the permission mode or sandbox the user chose for
  it, and prompt injection that stays inside one session's granted
  permissions;
- attacks that need code already running as the same OS user, unless Luna
  hands that code more than it already has;
- the behaviors listed as accepted in the threat model;
- denial of service that needs local access and only affects the attacker's
  own sessions.

## Threat model

The assets, trust boundaries, mitigations and accepted risks are documented in
[docs/security/threat-model.md](docs/security/threat-model.md).
