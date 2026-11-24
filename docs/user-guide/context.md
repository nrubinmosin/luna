# What Luna adds to a model's context

Luna puts text of its own into the context of the sessions it starts and
drives: tool descriptions, what a session is told of its role, notices, a
refused command's reason. This page lists every
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
  compaction.
- **Per event**: once each time the event happens.

The [delegates' auto-compaction setting](agents.md) controls when a helper
or lead compacts its context. Luna applies it at launch without adding prompt text.

## Totals by role

A session has one of three roles ([library](library.md#whose-sessions-chats-leads-helpers)):
your own **chat**; a **lead**, a helper started with Luna's tools; a
**helper**, started without them.

With default settings:

| Session | Every request | Once | Per event |
| --- | --- | --- | --- |
| Chat without **Luna tools** | 0 | 0 | 0 |
| Chat with **Luna tools** | ~2,370: the instructions and 16 tools | 0 | notices of the sessions it started or wrote to; guide topics it reads |
| Lead | ~2,420: the instructions, its 14 tools and its role text | the note after its task, ~42 (Codex: after every task) | the same as a chat |
| Helper | ~210: its role text | the note after its task, ~42 (Codex: after every task) | 0 |

Added to these where you turn them on: a role text you made longer goes with
every request of each lead or helper ([System texts](#what-a-session-is-told-of-its-role)).
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
| Instructions: what Luna is, how a spawn and its notice go, where the guide is | chats with Luna tools, leads; Claude Code and Codex | every request: Claude Code puts a server's instructions in its system prompt; whether Codex shows them to its model was not checked | 153 | the **Luna tools** box; a spawn's `tools`. The text: not editable yet (below) |
| Tool descriptions and their schemas: 16 for a chat; 14 for a lead, which is not offered `luna_notification_send` and `luna_power_set` (only a chat may call them) nor told of `scope` (it reaches only the sessions under it) | the same | every request. Claude Code: Luna asks it to keep its tools loaded, not behind its tool search, but for the six a session seldom calls (`luna_run_list`, `luna_account_list`, `luna_account_get`, `luna_guide_get`, `luna_notification_send`, `luna_power_set`), which wait there until the model looks one up. Codex: whether it defers any is its own choice | a chat's 2,215, a lead's 1,834: every schema listed, the deferred ones included, though Claude Code loads a deferred tool's schema only when its tool search finds it | the same. The text is the tools' contract with the model |
| `luna_guide_get` answers | whoever asks; a lead's index and sections leave out the two tools it is not offered | per call, as a tool result; nothing asks a session to read any | the index ~190; a topic's section about 300 to 1,300 ([Measured sizes](#measured-sizes)) | read only when the model asks |
| Tool results | the caller | per call | as large as what was asked: a message read is cut at 60,000 characters (~15,000 tokens); a wait or report gives the report and a count of the rest | the call's own arguments (`last`, `reply`, `since`) |

## What a session is told of its role

Each session Luna starts is told of its role in layers of its own, at the
level of an instructions file ([library](library.md#what-a-session-is-told-of-its-role)).
Claude Code reads them from a file of Luna's: a chat at the end of its system
prompt (`--append-system-prompt-file`), read as it starts or resumes; a helper
as the `CLAUDE.md` of a folder of Luna's (`--add-dir`). Either way they go with
every request. Codex gets them as `developer_instructions`: at a
conversation's start and after a compaction, so once per conversation, but
they stay in its context all along.

| Text | Who | When, where | ~Tokens | Change or turn off |
| --- | --- | --- | --- | --- |
| `role-lead`: answer the agent in its language and form, split work among helpers, own what they report, status words, constraints pass down, stop leftovers, no turn ended on "waiting" with nothing to wake it, keep caveats, send and install nothing unasked | leads | layers, as above | 436 | Library → System texts, `role-lead` |
| `role-helper`: the same, less what is about helpers | helpers | layers, as above | 210 | Library → System texts, `role-helper` |
| The note after a task (`[A note, not part of the task: …]`): the task comes from an agent; rules for a chat with a person do not apply | leads and helpers | Claude Code: after the first task, once; Codex: after every task, so it adds up in the conversation | 42 | Library → System texts, `note` (one for leads and helpers; one line; it cannot be empty) |
| Constraints: a heading, then each constraint word for word | a helper spawned with `constraints`, and every session under it | layers, last | 57 + the constraints | the agent that spawns; nothing removes one |
| The library's instructions for some roles: a heading `# Instructions: <name>`, then your file | the roles you ticked | layers | ~5 + your file | the library's **For** column |
| The line before instructions an earlier Luna laid out for chats only ("For chats with a person only: …") | every session of that account (Codex reads the account's `AGENTS.md` in all of them; Claude Code leaves it out of leads and helpers) | every request, with the file | 25 | tick another role for that file: it then goes per session instead |

## Skills

| Text | Who | When, where | ~Tokens | Change or turn off |
| --- | --- | --- | --- | --- |
| Your library's skills: each one's name and description, listed by the CLI; its body when the model loads it | the roles ticked in the **For** column, in the accounts and folders it reaches | the list every request; a body once, when loaded | yours | the library: **For** (`skillOverrides` for Claude Code, `skills.config` for Codex), accounts, folders. A Codex chat's pane reports any skills left visible by the command-line cap |

## Hooks

Luna's hooks (`UserPromptSubmit`, `Stop`, the rest) tell Luna what a session
does and answer with nothing, so they add nothing. One answer does reach a
model:

| Text | Who | When | ~Tokens | Change or turn off |
| --- | --- | --- | --- | --- |
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
| Mid-turn and quiet for 30 minutes (once a turn), with how to see its screen | 70 |
| Answered, but its CLI waits on what it left in the background | 125 |
| Its CLI stopped its background work (added to another line) | 60 |
| Its background subagents ended with its turn without finishing, by name (added to another line) | 50 + 8 a name |
| Did NOT finish: Luna's core restarted, or it stopped on an error | 40 to 60 |
| A question or a permission prompt, with its askId (plus the question) | 30 + the question |
| Stopped on a prompt Luna cannot read, with how to see it | 45 |
| A lead's delegates have finished (to the lead itself) | 85 |
| An account at 90% or 95% of a usage window | 35 |
| A helper past 85% of its context | 45 to 70 |
| The breaker: Luna stops telling of a session's turns (added once) | 40 |
| Context share past the level, added to each line about a helper: "(context N%)" | 4 |

`luna_session_list` gives compact rows with status and task information.

None of the text can be changed: a notice is status, and its words are what a
model acts on. What arrives can be shaped: `expectReply: false` on a send, a
spawn's `autoDelete` and `budgetPct` (0: no context share), **Tell me when a
helper's context passes** in Settings → Agents & trust → Helpers, and the
breaker, past 12 notices about one session in 15 minutes.

## Other text Luna types into a session

| Text | Who | ~Tokens |
| --- | --- | --- |
| `[From session <id> via Luna]` before a message from a session not above the one it goes to | one of your chats written to by another, with `scope: "all"` | 9 |
| A message over 8,000 characters goes into a file, and the session gets "This message is N characters long, so Luna put it in a file: …" | the session written to | 45 |
| After Luna's core restarted: "Luna's core restarted and stopped your last turn before it finished. Carry on where you left off." | a helper resumed by `resumeOnRestart` | 35 |

## Not editable yet, and why

The role texts and the note are system texts you
can change in the library. What stays Luna's own is mechanism, or too small and
too tied to a mechanism to be worth a setting yet:

- **The MCP instructions** (~145 tokens): what Luna is, what a notice looks
  like and where the guide is. A session reads them once, when its CLI connects,
  so an override would need a kind of system text of its own; most of what is
  left is the notice's tag and the meaning of ids.
- **Tool descriptions and schemas, refusals, the guide**: the contract between
  Luna and a model. The guide is read only when asked.
- **Notices**: status, read by the model to decide its next step.

## Measured sizes

Characters of each fixed text as it reaches a model, and tokens as characters
divided by four. The `per-request` rows add up what each role carries with
every request; they count every tool listed, the ones Claude Code defers
included. Checked by `context_page.rs`.

| Text | Characters | ~Tokens |
| --- | ---: | ---: |
| `instructions` | 613 | 153 |
| `tools` | 8,858 | 2,215 |
| `tools-lead` | 7,335 | 1,834 |
| `per-request-chat` | 9,471 | 2,368 |
| `per-request-lead` | 9,693 | 2,423 |
| `per-request-helper` | 831 | 208 |
| `role-lead` | 1,745 | 436 |
| `role-helper` | 831 | 208 |
| `note` | 167 | 42 |
| `constraints-heading` | 229 | 57 |
| `notice-tag` | 37 | 9 |
| `guide-index` | 758 | 190 |
| `guide-sessions` | 5,124 | 1,281 |
| `guide-accounts` | 1,266 | 317 |
| `guide-spawning` | 5,029 | 1,257 |
| `guide-getting` | 5,042 | 1,261 |
| `guide-answering` | 2,583 | 646 |
| `guide-restart` | 1,073 | 268 |
| `guide-working` | 2,246 | 562 |
