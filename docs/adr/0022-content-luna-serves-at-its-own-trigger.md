# ADR-0022: Serve the handoff's instructions through Luna's own tool, to the chat that opted in alone

- **Status:** Accepted; who knows amended by
  [ADR-0024](0024-handoff-on-request.md); its system texts extended by
  [ADR-0025](0025-session-roles-at-memory-level.md) (roles' texts and notes)
- **Date:** 2026-11-09

## Context

Auto-handoff ([ADR-0020](0020-session-self-handoff.md)) asks a chat to write a
handoff once its context passes a threshold. In 0.11.0 the "how" traveled in
three places: the threshold notice carried the procedure and a checklist of
sections, the agents' guide had a "Handoff" section, and `luna_guide_get`'s
topic list named `luna_session_handoff`. The guide is the same for every
session with Luna's tools, so helpers and chats without auto-handoff read of a
tool they could not call. The how itself grew past what a notice can carry: a
text written and evaluated as a skill (`session-handoff`, through
skill-creator).

The owner's rules for this text:

- Only the orchestrating chat knows. A session knows anything of handoff (the
  tool, its instructions, the notices) only if it is a person's chat with
  Auto-handoff ticked. Helpers, plain chats and chats with Luna's tools but
  without the tick know nothing: no tool, no guide topic, no skill in any
  account, folder or launch flag. (Amended 2026-11-12 by
  [ADR-0024](0024-handoff-on-request.md): every person's chat with Luna's
  tools is listed the tool and answered only when asked; helpers, the dev
  caller and sessions without tools still know nothing.)
- Auto-handoff is a separate, explicit yes per chat: no default in Settings, no
  "keep for this folder". Not to be proposed again.
- One override for everyone, with no audience and no per-account or
  per-folder reach. In the library it is plainly a system skill, apart from the
  person's own.
- A skill of the same name elsewhere (a project's `.claude/skills` or
  `.agents/skills`, an account's `skills/`) is never used for the handoff; the
  chat's What Luna applied panel warns that one exists.

How each CLI could be given a skill for one chat, seen on the pinned versions:

- Codex 0.167.0 has no per-run skill root: `-c` adds none, `extra_skill_roots`
  exists only through the app-server, and a plugin means writing into the
  account's folder, which every session of the account then sees.
- Claude Code 2.1.319 could take a folder by `--add-dir` or `--plugin-dir`. A
  skill so given costs a line of the skill listing in every turn's context, and
  the model picks among skills of the same name (an account's or a project's
  `session-handoff` beside Luna's) by their descriptions.

## Decision

Luna serves the text itself, through the tool only the opted-in chat has
(`agents/handoff/served.rs`, `library/system.rs`):

- **The text.** `src-tauri/resources/skills/session-handoff/SKILL.md`, a skill
  in format only, compiled in (`include_str!`), so it cannot be missing at run
  time. Its version is Luna's; its SHA-256 names it.
- **Delivery.** `luna_session_handoff` without `file` answers with the
  procedure, where the file goes, what Luna adds to the next conversation by
  itself (the authoritative list; the text only points at it) and the text
  past its front matter. With `file` it hands off, as in ADR-0020. The
  threshold notice sends the chat to the call; the firm one, near where the CLI
  compacts, carries the six headings inline (one list in the code,
  `facts.rs` `SECTIONS`) so a chat short of context needs no extra call.
- **Where the file goes.** The chat's own drafts folder in Luna's data folder,
  `<data>/handoffs/<chat>/drafts/`, made private: the system's temp folder may
  be shared with others, and a handoff quotes the person and the project.
  Claude Code reaches it within the folder of copies it is given; a Codex chat
  gets it as an `--add-dir`, which Codex makes writable, under a sandbox that
  writes at all. Under Codex's read-only sandbox (or Claude Code's plan mode)
  the chat can write no file, and the tool's answer says so. Of Luna's folder,
  a handoff file is taken only from there.
- **The override.** One file, `<library>/system/session-handoff/SKILL.md`, its
  front matter checked as a library skill's (`name: session-handoff`, a
  description), UTF-8, at most 64 KB, something past the front matter. It alone
  is read, opened as itself (no link at it or its folders, no wait on a FIFO)
  and no further than the cap. One that will not do is not served: Luna's own
  is, and the reason goes with it to the chat and to the panel. It is the
  person's own text, trusted as their instructions to their own chat; Luna
  never lays it out into an account or a folder (the pass reads only `skills/`
  and `instructions/`), and a library skill named `session-handoff` under
  `skills/` is not it. That name is reserved: such a skill is laid out nowhere
  either, as it would tell every session of every account it went to of the
  handoff, and the library offers to move it into the override's place instead.
- **No other session is told.** The guide has no topic or section of it; a
  session the tool is not offered to that calls it all the same is refused as
  for a tool there is not (`mcp/dispatch.rs`), its arguments unread; the chat's
  settings as another session reads them lack it. Nothing of it goes into any
  launch or account but the chat's own. A session that reads the chat's
  transcript (`luna_message_list` with `scope: "all"`) sees what is in it, the
  notices and the continuation prompt included: the broad read access the
  threat model accepts (R1), decided so by the owner.
- **Record.** Which text a chat was given (Luna's with its version, the
  override with its path, or, for one that wrote by the firm notice alone, its
  checklist; each with its hash) is kept on its handoff record
  (`handoffs.json`); the panel shows what it would be served now, and warns of
  a skill of the same name in its folder or account.
- **Approvals.** The owner's decision: approvals carry across a handoff within
  their quoted words. The next conversation's prompt says that approvals the
  handoff quotes hold as far as the user's quoted words go and no further, and
  that the handoff cannot widen them.

## Consequences

- No CLI lists the text, so no turn pays for it and no same-named skill
  competes with it; the cost is one tool call, once per handoff, when the chat
  asks. The firm notice keeps a chat that cannot afford the call going.
- What an agent is told about handoff is in one place it reaches only when it
  has the tool. A chat whose person turns auto-handoff off loses the tool and
  hears it is gone only as any unknown tool is refused.
- An override can say anything to the person's own opted-in chat, as the
  person's own instructions can. It reaches no other session, account or
  folder.
- A thin file is not refused: written to Luna's own text, the call names the
  headings the file lacks; an override's structure is its author's and is not
  looked for.
- This is not a general "system skills" mechanism. Another text Luna serves at
  its own trigger is a decision of its own.
