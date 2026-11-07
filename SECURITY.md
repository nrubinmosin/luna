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
- how Luna downloads, verifies, installs and selects the CLI binaries it runs;
- the WebView UI and its Tauri commands.

Out of scope:

- vulnerabilities in Claude Code, Codex, the model APIs or other dependencies
  themselves (report them upstream; a way Luna makes one worse is in scope);
- what an agent does within the permission mode or sandbox the user chose for
  it, and prompt injection that stays inside one session's granted
  permissions;
- what a session does through Luna's tools within the authority they grant.
  Enabling Luna's tools for a session grants it orchestration authority
  bounded by its own privilege; escalation happens only through an explicit
  user setting (`helpersAboveCaller`). See "Trust contract" and "Limits of the
  least-privilege layer" in the threat model. A way for a session to start a
  helper above its own permission mode or sandbox with that setting off, to
  approve a permission prompt or type keys outside its own helpers, or
  otherwise to break that contract, is in scope;
- attacks that need unsandboxed code already running as the same OS user,
  unless Luna hands that code more than it already has;
- the risks the threat model lists as accepted (R1-R7) and the documented
  limits of the least-privilege layer; its residual findings (F3, F9, F10)
  are not out of scope;
- denial of service that needs local access and only affects the attacker's
  own sessions.

## Threat model

The assets, trust boundaries, the status of every finding and the accepted
risks are documented in
[docs/security/threat-model.md](docs/security/threat-model.md).
