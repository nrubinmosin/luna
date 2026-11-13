# What Luna adds to a model's context

Luna puts text of its own into the context of the sessions it starts and
drives: tool descriptions, what a session is told of its role, notices, a
refused command's reason, a handoff's instructions. This page lists every
piece, who gets it, when, what it costs and how to change it or turn it off.
Nothing else Luna writes reaches a model.

How to read the numbers:

- **Tokens are characters divided by four**, measured on the real text (a
  path or an answer quoted in it counts at a typical length). They are an
  estimate, not a tokenizer's count. The fixed texts are in
  [Measured sizes](#measured-sizes) below, which a unit test
  (`src-tauri/src/context_page.rs`) checks against the code: a change that
  grows or cuts one fails until this page says so.
- **Every request**: sent with each request to the model, for as long as the
  session runs. Prompt caching makes a repeat cheaper, but it still takes room
  in the context window.
- **Once**: enters the conversation once and stays in its history until a
  compaction or a handoff.
- **Per event**: once each time the event happens.

## Totals by role

A session has one of three roles ([library](library.md#whose-sessions-chats-leads-helpers)):
your own **chat**; a **lead**, a helper started with Luna's tools; a
**helper**, started without them.

| Session | Every request | Once | Per event |
| --- | --- | --- | --- |
| Chat without **Luna tools** | 0 | 0 | 0 |
| Chat with **Luna tools** | ~2,465: the instructions, 16 tools and the handoff's tool | 0 | notices of the sessions it started or wrote to; guide topics it reads; a handoff when asked |
| Lead | ~2,825: the chat's, plus its role text | the note after its task, ~65 (Codex: after every task) | the same as a chat; a handoff when its context passes the lead threshold |
| Helper | ~210: its role text | the note after its task, ~65 (Codex: after every task) | a refused command, ~125 |

On top of these, every session reads what you put in: your instructions files
and skills (by role, through the library's **For** column), and a helper's
**constraints** from the agents above it. Luna adds a heading of a few words to
each, and to the constraints a line saying they only narrow the rest (~57 in all).

The same numbers before Luna 0.18, for comparison: a chat with Luna tools
~2,655 every request, and a conversation that spawned was told to read two
guide topics, ~2,200 tokens, before its first spawn; a lead ~3,050.

## The MCP server: instructions and tools

Only a session with Luna's tools has Luna as an MCP server it can use: a chat
with **Luna tools** ticked (off by default), a lead (a helper spawned with
`tools: true`). Nobody else is told of it.

| Text | Who | When, where | ~Tokens | Change or turn off |
| --- | --- | --- | --- | --- |
| Instructions: what Luna is, how a spawn and its notice go, where the guide is | chats with Luna tools, leads; Claude Code and Codex | every request: Claude Code puts a server's instructions in its system prompt; whether Codex shows them to its model was not checked | 145 | the **Luna tools** box; a spawn's `tools`. The text: not editable yet (below) |
| 16 tool descriptions and their schemas | the same | every request (a CLI may defer tools behind its own tool search; Luna does not ask it to) | 2,220 | the same. The text is the tools' contract with the model |
| `luna_session_handoff` | every chat with Luna tools, auto-handoff on or not; a lead while **Auto-handoff for leads** is on (on by default) | every request | 100 | for a lead, that setting (Settings → Agents & trust → Helpers); for a chat, only the **Luna tools** box ([ADR-0024](../adr/0024-handoff-on-request.md)) |
| `luna_permission` | a background Claude Code helper | hidden from its model (`--disallowedTools`); its CLI calls it for permission prompts | 0 | nothing to turn off |
| `luna_guide_get` answers | whoever asks | per call, as a tool result; nothing asks a session to read any | the index ~190; a topic's section 300 to 1,200 ([Measured sizes](#measured-sizes)) | read only when the model asks |
| Tool results | the caller | per call | as large as what was asked: a message read is cut at 60,000 characters (~15,000 tokens); a wait or report gives the report and a count of the rest | the call's own arguments (`last`, `reply`, `since`) |

## What a session is told of its role

Each session Luna starts is told of its role in layers of its own, at the
level of an instructions file ([library](library.md#what-a-session-is-told-of-its-role)).
Claude Code reads them as the `CLAUDE.md` of a folder of Luna's
(`--add-dir`): every request. Codex gets them as `developer_instructions`: at a
conversation's start and after a compaction, so once per conversation, but
they stay in its context all along.

| Text | Who | When, where | ~Tokens | Change or turn off |
| --- | --- | --- | --- | --- |
| `role-chat` | chats | nothing unless you write one | 0 | Library → System texts |
| `role-lead`: answer the agent in its language and form, split work among helpers, own what they report, status words, constraints pass down, stop leftovers, keep caveats, send and install nothing unasked | leads | layers, as above | 360 | Library → System texts, `role-lead` |
| `role-helper`: the same, less what is about helpers | helpers | layers, as above | 210 | Library → System texts, `role-helper` |
| The note after a task (`[A note, not part of the task: …]`): the task comes from an agent; rules for a chat with a person do not apply | leads and helpers | Claude Code: after the first task, once; Codex: after every task, so it adds up in the conversation | 65 | Library → System texts, `note-lead`, `note-helper` (one line; it cannot be empty) |
| Constraints: a heading, then each constraint word for word | a helper spawned with `constraints`, and every session under it | layers, last | 57 + the constraints | the agent that spawns; nothing removes one |
| The library's instructions for some roles: a heading `# Instructions: <name>`, then your file | the roles you ticked | layers | ~5 + your file | the library's **For** column |
| The line before instructions an earlier Luna laid out for chats only ("For chats with a person only: …") | every session of that account (Codex reads the account's `AGENTS.md` in all of them; Claude Code leaves it out of leads and helpers) | every request, with the file | 25 | tick another role for that file: it then goes per session instead |

## Skills

| Text | Who | When, where | ~Tokens | Change or turn off |
| --- | --- | --- | --- | --- |
| Your library's skills: each one's name and description, listed by the CLI; its body when the model loads it | the roles ticked in the **For** column, in the accounts and folders it reaches | the list every request; a body once, when loaded | yours | the library: **For** (`skillOverrides` for Claude Code, `-c skills.config` for Codex), accounts, folders |
| `session-handoff`, the handoff's instructions | a chat or lead asked for a handoff, as the tool's answer; no CLI lists it | once per handoff or move | 1,070 | Library → System texts, `session-handoff` |

## Hooks

Luna's hooks (`UserPromptSubmit`, `Stop`, the rest) tell Luna what a session
does and answer with nothing, so they add nothing. Two answers do reach a
model:

| Text | Who | When | ~Tokens | Change or turn off |
| --- | --- | --- | --- | --- |
| The guard's refusal: a command that follows a file (`tail -f`, `Get-Content -Wait`) with no deadline, what to do instead | every lead and helper; a chat with **Wait guard** ticked; Codex only where its hooks run unasked | per refused command | 125 | for a chat, the **Wait guard** box. For a helper, nothing: such a wait outlives the tool that ran it and keeps the PC awake all night. The text: not editable yet |
| A permission prompt's answer: "Nobody answered in time; go on without it or ask in your reply.", "The user declined." or the note the answering agent gave | a background Claude Code helper | per prompt | 5 to 16, or the note | the answering agent's `note` |

## Notices

A notice is a line Luna types into a session that started or wrote to another:
it opens with `[Luna notice, not typed by the user]` (~9 tokens), and the lines
of one moment go in as one message. Only a chat with Luna tools or a lead gets
them. Each costs the session a turn as well as its tokens.

| Notice | Typical ~tokens |
| --- | --- |
| Finished, its answer quoted (300 characters at most) | 85 |
| Finished, its answer in a file: the path, the report's lines, its first line | 100 |
| NOT done yet: a job, subagent or helper of its still runs (said once) | 135 |
| Nothing left running, and it said nothing since | 50 |
| Only idle processes left, with what they were started as | 85 |
| Mid-turn and quiet for 30 minutes (once a turn), with how to see its screen | 70 |
| Answered, but its CLI waits on what it left in the background | 125 |
| Its CLI stopped its background work (added to another line) | 60 |
| Did NOT finish: Luna's core restarted, or it stopped on an error | 40 to 60 |
| A question or a permission prompt, with its askId (plus the question) | 30 + the question |
| Stopped on a prompt Luna cannot read, with how to see it | 45 |
| A lead's delegates have finished (to the lead itself) | 85 |
| An account at 90% or 95% of a usage window | 35 |
| A helper past 85% of its context | 45 to 70 |
| The breaker: Luna stops telling of a session's turns (added once) | 40 |
| Context share past the level, added to each line about a helper: "(context N%)" | 4 |

None of the text can be changed: a notice is status, and its words are what a
model acts on. What arrives can be shaped: `expectReply: false` on a send, a
spawn's `autoDelete` and `budgetPct` (0: no context share), **Tell me when a
helper's context passes** in Settings → Agents & trust → Helpers, and the
breaker, past 12 notices about one session in 15 minutes.

## Handoff and move

A chat with Luna tools hands off when **auto-handoff** or **Hand off now** asks
it to, and writes a document when you move it to another account; a lead hands
off while **Auto-handoff for leads** is on. Nothing here reaches another session.

| Text | When, where | ~Tokens | Change or turn off |
| --- | --- | --- | --- |
| The ask: the context share, then "luna_session_handoff without arguments says how" | once at the threshold, as a notice; the person's **Hand off now** says the same | 45 | the auto-handoff settings |
| The firm ask near compaction, with the file's headings inline (Luna's own instructions in effect; else as short as the first) | once, as a notice | 285 | the same |
| The tool's answer: the steps, where the file goes, what Luna adds itself, then `session-handoff` | once per handoff, as a tool result | 300 + 1,070 | `session-handoff` in Library → System texts. The steps: not editable, they are the tool's contract |
| The next conversation's first prompt: where the handoff is, load its skills, approvals go no further than quoted; then Luna's facts (helpers, debts, accounts, settings) | once, as its first message | ~180 + the facts | the handoff file is the chat's own |
| A move's ask, then the tool's answer with the move's rules | once each | 70; 375 + 1,070 | the same |

A chat's next conversation starts with `/clear` (Claude Code) or `/new`
(Codex) typed into it, a lead's with a fresh run of its CLI: no text for the
model either way.

## Other text Luna types into a session

| Text | Who | ~Tokens |
| --- | --- | --- |
| `[From session <id> via Luna]` before a message from a session that is not the parent | a session written to with `scope: "all"` | 9 |
| A message over 8,000 characters goes into a file, and the session gets "This message is N characters long, so Luna put it in a file: …" | the session written to | 45 |
| After Luna's core restarted: "Luna's core restarted and stopped your last turn before it finished. Carry on where you left off." | a helper resumed by `resumeOnRestart` | 35 |

## Not editable yet, and why

The role texts, the notes and the handoff's instructions are system texts you
can change in the library. What stays Luna's own is mechanism, or too small and
too tied to a mechanism to be worth a setting yet:

- **The MCP instructions** (~145 tokens): what Luna is, what a notice looks
  like and where the guide is. A session reads them once, when its CLI connects,
  so an override would need a kind of system text of its own; most of what is
  left is the notice's tag and the meaning of ids.
- **Tool descriptions and schemas, refusals, the guide**: the contract between
  Luna and a model. The guide is read only when asked.
- **Notices**: status, read by the model to decide its next step.
- **The guard's refusal** (~125 tokens): says what to do instead of the
  refused command, in bash and in PowerShell, whose `timeout` only waits.
- **The handoff's and move's asks and steps**: the tool's contract; the
  instructions they wrap are a system text.

## Measured sizes

Characters of each fixed text as it reaches a model, and tokens as characters
divided by four. The `per-request` rows add up what each role carries with
every request. Checked by `context_page.rs`.

| Text | Characters | ~Tokens |
| --- | ---: | ---: |
| `instructions` | 578 | 145 |
| `tools` | 8,886 | 2,222 |
| `handoff-tool` | 390 | 98 |
| `per-request-chat` | 9,854 | 2,464 |
| `per-request-lead` | 11,292 | 2,823 |
| `per-request-helper` | 831 | 208 |
| `role-lead` | 1,438 | 360 |
| `role-helper` | 831 | 208 |
| `note` | 265 | 66 |
| `constraints-heading` | 229 | 57 |
| `notice-tag` | 37 | 9 |
| `guard` | 490 | 123 |
| `handoff-skill` | 4,278 | 1,070 |
| `handoff-ask` | 179 | 45 |
| `handoff-ask-firm` | 1,148 | 287 |
| `handoff-how-to` | 1,193 | 298 |
| `move-ask` | 277 | 69 |
| `move-how-to` | 1,501 | 375 |
| `guide-index` | 758 | 190 |
| `guide-sessions` | 4,770 | 1,193 |
| `guide-accounts` | 1,266 | 317 |
| `guide-spawning` | 4,480 | 1,120 |
| `guide-getting` | 4,400 | 1,100 |
| `guide-answering` | 2,180 | 545 |
| `guide-restart` | 1,189 | 297 |
| `guide-working` | 1,779 | 445 |
