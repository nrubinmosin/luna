import { useEffect, useState } from 'react';
import { createPortal } from 'react-dom';
import { appliedByLuna, type Applied, type AppliedClass, type AppliedView } from '../../ipc/applied';
import type { Chat } from '../../shared/types';
import { useChats } from './chats.store';

/** The classes in the order they are shown, each with what it means. */
export const CLASSES: { cls: AppliedClass; title: string; means: string }[] = [
  {
    cls: 'userAction',
    title: 'You chose',
    means: 'In the new-chat dialog, Settings, the library, or an agent you gave tools.'
  },
  { cls: 'runtime', title: 'Luna needs it to run the chat', means: 'What makes the CLI one of Luna’s sessions.' },
  { cls: 'boundary', title: 'Protects Luna’s own boundary', means: 'Keeps the session out of Luna’s secrets.' },
  { cls: 'workaround', title: 'Works around the CLI on this machine', means: 'What the CLI cannot do here without it.' }
];

/** `items` by class, in `CLASSES` order, empty classes left out. */
export function byClass(items: Applied[]): { cls: AppliedClass; title: string; means: string; items: Applied[] }[] {
  return CLASSES.map(c => ({ ...c, items: items.filter(a => a.class === c.cls) })).filter(g => g.items.length > 0);
}

function Group({ title, means, items }: { title: string; means: string; items: Applied[] }) {
  return (
    <section style={{ marginTop: 10 }}>
      <div style={{ fontWeight: 600, fontSize: 'var(--fs-4)', color: 'var(--fg)' }}>{title}</div>
      <div style={{ fontSize: 'var(--fs-2)', color: 'var(--faint)', marginBottom: 4 }}>{means}</div>
      {items.map((a, i) => (
        <div key={`${a.key}-${i}`} style={{ padding: '4px 0', borderTop: '1px solid var(--line)' }}>
          <div style={{ fontSize: 'var(--fs-3)', color: 'var(--dim)', fontWeight: 600 }}>{a.key}</div>
          <div
            title={a.value}
            style={{
              fontFamily: 'ui-monospace, Consolas, monospace',
              fontSize: 'var(--fs-2)',
              color: 'var(--fg)',
              whiteSpace: 'pre-wrap',
              wordBreak: 'break-all'
            }}
          >
            {a.value}
          </div>
          <div style={{ fontSize: 'var(--fs-2)', color: 'var(--faint)' }}>{a.why}</div>
        </div>
      ))}
    </section>
  );
}

/**
 * The chat's own "Wait guard", as the new-chat dialog has it: for a chat
 * made before it was a choice, or to change one's mind. Applies from the
 * CLI's next start. A helper has the guard whatever this says, so it gets no
 * box; Codex runs the hook only with its hooks unasked.
 */
function GuardBox({ chat }: { chat: Chat }) {
  const on = useChats(s => s.findChat(chat.id)?.guard ?? false);
  if (chat.parentId) return null;
  const able = chat.provider !== 'codex' || !!chat.hooksUnasked;
  return (
    <div style={{ marginTop: 10 }}>
      <div
        className="field-row"
        title={able ? undefined : 'Codex runs a hook Luna gives it only with Hooks unasked on'}
      >
        <input
          type="checkbox"
          id={`guard-${chat.id}`}
          checked={on && able}
          disabled={!able}
          onChange={() => useChats.getState().setGuard(chat.id, !on)}
        />
        <label htmlFor={`guard-${chat.id}`} style={{ cursor: 'default' }}>
          Wait guard for this chat
        </label>
      </div>
      <div style={{ fontSize: 'var(--fs-2)', color: 'var(--faint)', marginTop: 2 }}>
        Refuses a shell command that would wait forever. Takes effect when the chat&apos;s CLI next starts.
      </div>
    </div>
  );
}

/**
 * What Luna applied to a chat's CLI, read-only: its latest start grouped by
 * why each item is there, and what Luna wrote into its account outside a
 * start. Asked of the core as it opens (applied.rs `applied_by_luna`).
 */
export function AppliedPanel({ chat, onClose }: { chat: Chat; onClose: () => void }) {
  const [view, setView] = useState<AppliedView | null>(null);
  useEffect(() => {
    let stale = false;
    void appliedByLuna(chat.id)
      .then(v => !stale && setView(v))
      .catch(() => !stale && setView({ launch: [], account: [], incomplete: null }));
    return () => {
      stale = true;
    };
  }, [chat.id]);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);

  // Into the app root, as ConfirmDialog does: the title bar it opens from is
  // draggable, and a dialog inside it would take no clicks.
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
        backdropFilter: 'blur(1px)',
        display: 'grid',
        placeItems: 'center',
        zIndex: 80
      }}
    >
      <div
        onClick={e => e.stopPropagation()}
        role="dialog"
        aria-label="What Luna applied"
        className="window"
        style={{ width: 560, boxShadow: 'var(--shadow), var(--border-window-outer), var(--border-window-inner)' }}
      >
        <div className="title-bar">
          <div className="title-bar-text">What Luna applied — {chat.name}</div>
          <div className="title-bar-controls">
            <button aria-label="Close" onClick={onClose} />
          </div>
        </div>
        <div className="window-body" style={{ maxHeight: '70vh', overflowY: 'auto' }}>
          <div style={{ fontSize: 'var(--fs-3)', color: 'var(--dim)', lineHeight: 1.45 }}>
            Luna adds to a CLI only what it needs to run it, what keeps its own secrets safe, and what you chose. Tokens
            and Luna&apos;s local addresses are not shown; files are named by their path.
          </div>
          <GuardBox chat={chat} />
          {view?.incomplete && (
            <div
              role="alert"
              style={{
                marginTop: 10,
                padding: '6px 8px',
                border: '1px solid oklch(.62 .17 60)',
                borderRadius: 2,
                fontSize: 'var(--fs-3)',
                color: 'var(--fg)'
              }}
            >
              <b>Accounting incomplete.</b> Luna may have written into this account more than is listed below:{' '}
              <span style={{ fontFamily: 'ui-monospace, Consolas, monospace', whiteSpace: 'pre-wrap' }}>
                {view.incomplete}
              </span>
            </div>
          )}
          {view === null ? (
            <div style={{ marginTop: 10, color: 'var(--faint)' }}>Asking the core…</div>
          ) : view.launch.length === 0 && view.account.length === 0 ? (
            <div style={{ marginTop: 10, color: 'var(--faint)' }}>
              Nothing yet: the chat has not started under this Luna.
            </div>
          ) : (
            <>
              {byClass(view.launch).map(g => (
                <Group key={g.cls} title={g.title} means={g.means} items={g.items} />
              ))}
              {view.account.length > 0 && (
                <Group
                  title={`Written into the account “${chat.account}”`}
                  means="Outside a start: in the account's own files, where the CLI reads them."
                  items={view.account}
                />
              )}
            </>
          )}
          <div style={{ marginTop: 14, display: 'flex', justifyContent: 'flex-end' }}>
            <button onClick={onClose}>Close</button>
          </div>
        </div>
      </div>
    </div>,
    host
  );
}
