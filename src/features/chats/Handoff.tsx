/**
 * A chat's handoffs as the window shows them (agents/handoff): the box that
 * sets auto-handoff on a chat that runs, the divider over its terminal that
 * says where its latest handoff stands, and what the divider opens — Luna's
 * copy of the handoff, and the conversation it left, read-only.
 */
import { useEffect, useState } from 'react';
import { createPortal } from 'react-dom';
import type { HandoffApplied, SystemSkillState } from '../../ipc/applied';
import { handoffFile, handoffTranscript } from '../../ipc/handoff';
import { HANDOFF_AT, type Chat, type HandoffAsked, type HandoffMark } from '../../shared/types';
import { ConversationView } from '../agents/HelperView';
import { useHelperTranscript } from '../agents/helperTranscript';
import { useRestarts } from '../panes/restarts.store';
import { applyAutoHandoff } from './autoHandoff';
import { useChats } from './chats.store';

const noteStyle = { fontSize: 'var(--fs-2)', color: 'var(--faint)', marginTop: 2 } as const;

/** A threshold as typed, held to what the window offers. */
export function clampThreshold(v: number): number {
  if (!Number.isFinite(v)) return HANDOFF_AT.default;
  return Math.min(HANDOFF_AT.max, Math.max(HANDOFF_AT.min, Math.round(v)));
}

/**
 * The threshold box: a number in percent, taken as soon as it is one in the
 * range the window offers (an Enter that creates the chat comes before any
 * blur would), and held to that range when it is left.
 */
export function ThresholdInput({
  id,
  value,
  disabled,
  onCommit
}: {
  id: string;
  value: number;
  disabled: boolean;
  onCommit: (v: number) => void;
}) {
  const [text, setText] = useState(String(value));
  // A value set from elsewhere replaces what was typed, as the render sees it.
  const [shown, setShown] = useState(value);
  if (shown !== value) {
    setShown(value);
    setText(String(value));
  }
  const commit = () => {
    const v = clampThreshold(Number(text));
    setText(String(v));
    if (v !== value) onCommit(v);
  };
  return (
    <input
      id={id}
      type="number"
      aria-label="Auto-handoff threshold, percent of the context"
      min={HANDOFF_AT.min}
      max={HANDOFF_AT.max}
      step={5}
      value={text}
      disabled={disabled}
      onChange={e => {
        setText(e.target.value);
        const v = Number(e.target.value);
        if (Number.isInteger(v) && v >= HANDOFF_AT.min && v <= HANDOFF_AT.max && v !== value) onCommit(v);
      }}
      onBlur={commit}
      onKeyDown={e => {
        if (e.key === 'Enter') commit();
      }}
      style={{ width: 52, height: 'calc(var(--ui) * 1.6)' }}
    />
  );
}

/** Whose instructions a chat with auto-handoff on is served to write its
 *  handoff by, as the What Luna applied panel says it. */
export function instructionsWords(s: SystemSkillState): string {
  const sha = s.sha256.slice(0, 12);
  switch (s.state) {
    case 'library':
      return `yours (${s.path}, sha ${sha})`;
    case 'broken':
      return `yours is broken: ${s.problem ?? 'it will not do'} — Luna’s used (${s.version}, sha ${sha})`;
    default:
      return `Luna’s (${s.version}, sha ${sha})`;
  }
}

/** What a chat's handoff chip says of a handoff it was asked for and has not
 *  made: the share of its context at each ask, and its CLI's compactions since. */
export function askedWords(a: HandoffAsked): string {
  const at = a.at.map(p => `${p}%`);
  const list = at.length > 1 ? `${at.slice(0, -1).join(', ')} and ${at[at.length - 1]}` : (at[0] ?? '');
  const compacted =
    a.compacted === 0
      ? ''
      : `; its CLI compacted the conversation ${a.compacted === 1 ? 'once' : `${a.compacted} times`} before a handoff`;
  return `Handoff asked at ${list}, not done${compacted}`;
}

/**
 * The chat's own auto-handoff, as the new-chat dialog has it, for a chat
 * made without it or to change one's mind while it runs. Takes effect in Luna
 * at once; the tool reaches a running CLI by a quick resume of the chat, on
 * the same conversation, once it is idle. With it on, `instructions` (the
 * core's word, asked again through `onApplied` once the core has taken a
 * change made here) says whose instructions the chat is served for its
 * handoff, and warns of a skill of the same name its CLI may show the model,
 * which Luna never serves.
 */
export function HandoffBox({
  chat,
  instructions = null,
  onApplied
}: {
  chat: Chat;
  instructions?: HandoffApplied | null;
  onApplied?: () => void;
}) {
  const c = useChats(s => s.findChat(chat.id)) ?? chat;
  const pending = useRestarts(s => !!s.pending[chat.id]);
  const on = !!c.autoHandoff;
  const at = c.handoffAt ?? HANDOFF_AT.default;
  const able = !!c.tools;
  return (
    <div style={{ marginTop: 10 }}>
      <div
        className="field-row"
        title={able ? undefined : 'Only for a chat with Luna tools: their server is what offers the handoff'}
      >
        <input
          type="checkbox"
          id={`handoff-${chat.id}`}
          checked={on && able}
          disabled={!able}
          onChange={() => void applyAutoHandoff(chat.id, !on, at).then(onApplied)}
        />
        <label htmlFor={`handoff-${chat.id}`} style={{ cursor: 'default' }}>
          Auto-handoff at
        </label>
        <ThresholdInput
          id={`handoff-at-${chat.id}`}
          value={at}
          disabled={!able || !on}
          onCommit={v => void applyAutoHandoff(chat.id, on, v).then(onApplied)}
        />
        <span style={{ fontSize: 'var(--fs-3)', color: 'var(--dim)' }}>% of its context</span>
      </div>
      <div style={noteStyle}>
        Past it, Luna asks this chat to write a handoff; on its word the chat goes on in a fresh conversation from it,
        its helpers and their answers carrying on. Nothing switches unless the chat asks. Turned on while the chat runs,
        it takes effect at a quick resume of the chat, on the same conversation, once it is idle
        {pending ? ' — waiting for it to be idle.' : '.'}
      </div>
      {on && able && instructions && (
        <>
          <div style={{ ...noteStyle, color: 'var(--dim)' }}>
            Handoff instructions: {instructionsWords(instructions.instructions)}
          </div>
          {instructions.sameNamed.length > 0 && (
            <div
              role="alert"
              style={{
                marginTop: 6,
                padding: '6px 8px',
                border: '1px solid oklch(.62 .17 60)',
                borderRadius: 2,
                fontSize: 'var(--fs-3)',
                color: 'var(--fg)'
              }}
            >
              <b>A skill named {instructions.instructions.name} is here too.</b> The CLI may show it to the model; Luna
              serves its own instructions (or yours in the library) for the handoff, never this one:{' '}
              <span style={{ fontFamily: 'ui-monospace, Consolas, monospace', whiteSpace: 'pre-wrap' }}>
                {instructions.sameNamed.join('\n')}
              </span>
            </div>
          )}
        </>
      )}
    </div>
  );
}

const clock = (ms: number) => new Date(ms).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });

/** What the divider says of a handoff, its links aside. */
export function markWords(mark: HandoffMark): string {
  if (mark.moved)
    return mark.state === 'failed'
      ? `The move here did not go through: ${mark.why ?? 'Luna typed nothing'} · ${clock(mark.at)}`
      : `Moved here from another account · ${clock(mark.at)}`;
  switch (mark.state) {
    case 'underWay':
      return `Handoff ${mark.n} · under way since ${clock(mark.at)}`;
    case 'failed':
      return `Handoff ${mark.n} did not go through · ${clock(mark.at)}`;
    default:
      return `Handoff ${mark.n} · ${clock(mark.at)}`;
  }
}

/**
 * The divider over a chat's terminal once it has had a handoff: one row for
 * the chat, whose conversation goes on from here, with Luna's copy of the
 * handoff and the conversation before it a click away.
 */
export function HandoffBar({ chat }: { chat: Chat }) {
  const [open, setOpen] = useState<'file' | 'previous' | null>(null);
  const mark = chat.handoff;
  if (!mark) return null;
  const link = (what: 'file' | 'previous', words: string) => (
    <span role="button" tabIndex={0} className="handoff-link hover-bg" onClick={() => setOpen(what)}>
      {words}
    </span>
  );
  return (
    <div className="handoff-bar" data-state={mark.state} role="note" title={mark.why ?? undefined}>
      <span className="handoff-rule" />
      <span>{markWords(mark)}</span>
      <span aria-hidden>·</span>
      {link('file', mark.moved ? 'open document' : 'open handoff')}
      {mark.state === 'done' && mark.previous && (
        <>
          <span aria-hidden>·</span>
          {link('previous', 'previous conversation')}
        </>
      )}
      <span className="handoff-rule" />
      {open === 'file' && <HandoffFileView chat={chat} n={mark.n} onClose={() => setOpen(null)} />}
      {open === 'previous' && <PreviousConversation chat={chat} n={mark.n} onClose={() => setOpen(null)} />}
    </div>
  );
}

/** Luna's copy of handoff `n`, as text, read-only. */
export function HandoffFileView({ chat, n, onClose }: { chat: Chat; n: number; onClose: () => void }) {
  const [text, setText] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let stale = false;
    handoffFile(chat.id, n)
      .then(t => !stale && setText(t))
      .catch((e: unknown) => !stale && setError(String(e)));
    return () => {
      stale = true;
    };
  }, [chat.id, n]);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);
  const host = document.querySelector('[data-app]') ?? document.body;
  return createPortal(
    <div
      onClick={e => {
        e.stopPropagation();
        onClose();
      }}
      className="scrim"
      style={{
        position: 'fixed',
        inset: 0,
        background: 'rgba(0,0,0,.35)',
        display: 'grid',
        placeItems: 'center',
        zIndex: 80
      }}
    >
      <div
        onClick={e => e.stopPropagation()}
        role="dialog"
        aria-label={`Handoff ${n} of ${chat.name}`}
        className="window"
        style={{
          width: 'min(760px, 92vw)',
          height: 'min(640px, 86vh)',
          boxShadow: 'var(--shadow), var(--border-window-outer), var(--border-window-inner)'
        }}
      >
        <div className="title-bar">
          <div className="title-bar-text">
            Handoff {n} — {chat.name}
          </div>
          <div className="title-bar-controls">
            <button aria-label="Close" onClick={onClose} />
          </div>
        </div>
        <div className="window-body" style={{ display: 'flex', flexDirection: 'column' }}>
          <div style={{ fontSize: 'var(--fs-1)', color: 'var(--dim)', marginBottom: 6 }}>
            Luna&apos;s copy, as the chat wrote it when it handed off: what its next conversation read first.
          </div>
          <pre
            style={{
              flex: '1 1 auto',
              margin: 0,
              overflow: 'auto',
              background: 'var(--card)',
              border: '1px solid var(--line)',
              padding: 8,
              whiteSpace: 'pre-wrap',
              overflowWrap: 'anywhere',
              color: 'var(--fg)',
              fontSize: 'var(--fs-3)',
              fontFamily: 'ui-monospace, Consolas, monospace'
            }}
          >
            {error ? `Could not read it: ${error}` : (text ?? 'reading…')}
          </pre>
        </div>
      </div>
    </div>,
    host
  );
}

/** The conversation handoff `n` left, read-only; an earlier one a step back. */
export function PreviousConversation({ chat, n, onClose }: { chat: Chat; n: number; onClose: () => void }) {
  const [at, setAt] = useState(n);
  const t = useHelperTranscript(`${chat.id}#${at}`, true, since => handoffTranscript(chat.id, at, since));
  const status = t.error ? `cannot be read: ${t.error}` : t.loading ? 'reading…' : 'read-only, as it ended';
  return (
    <ConversationView
      title={`${chat.name} — before handoff ${at}`}
      label={`${chat.name}, before handoff ${at}, read-only`}
      provider={chat.provider}
      status={status}
      t={t}
      onClose={onClose}
      header={
        at > 1 ? (
          <div style={{ fontSize: 'var(--fs-1)' }}>
            <span role="button" tabIndex={0} className="handoff-link hover-bg" onClick={() => setAt(a => a - 1)}>
              ‹ before handoff {at - 1}
            </span>
          </div>
        ) : undefined
      }
    />
  );
}
