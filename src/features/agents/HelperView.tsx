import { useEffect, useLayoutEffect, useRef, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import type { BackgroundHelperDto } from '../../ipc/agents';
import { Icon } from '../../shared/ui/icons';
import type { Provider } from '../../shared/types';
import { useHelperTranscript, type HelperTranscript } from './helperTranscript';

/** Who said a message, as a person reads it. */
const ROLE: Record<string, string> = { user: 'Asked', assistant: 'Said' };

/**
 * A background helper's conversation, read-only, live while its turn runs: what
 * its agent asked of it and what it said, tool calls marked. Nothing here types
 * into it; Open as chat is how a person takes it over (bridge.ts `openAsChat`).
 */
export function HelperView({ helper, onClose }: { helper: BackgroundHelperDto; onClose: () => void }) {
  const t = useHelperTranscript(helper.id);
  const status = t.error
    ? `no longer a helper: ${t.error}`
    : t.loading
      ? 'reading…'
      : t.running
        ? 'working · updates as it goes'
        : 'between turns';
  return (
    <ConversationView
      title={`${helper.name || helper.id} — read-only`}
      label={`${helper.name || helper.id}, read-only`}
      provider={helper.provider}
      status={status}
      t={t}
      onClose={onClose}
    />
  );
}

/**
 * A conversation read-only, a helper's or one a chat's handoff left: its
 * messages, who said each, with the newest followed while the reader is at
 * the end. Escape or a click outside closes it.
 */
export function ConversationView({
  title,
  label,
  provider,
  status,
  t,
  onClose,
  header
}: {
  title: string;
  label: string;
  provider: Provider;
  status: string;
  t: HelperTranscript;
  onClose: () => void;
  /** Above the status line: what else the view offers. */
  header?: ReactNode;
}) {
  const list = useRef<HTMLDivElement>(null);
  // Following the end while the reader is at it; a reader scrolled up to read
  // something is left where they are.
  const atEnd = useRef(true);
  useLayoutEffect(() => {
    const el = list.current;
    if (el && atEnd.current) el.scrollTop = el.scrollHeight;
  }, [t.messages]);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);

  // As ConfirmDialog: out of the row's draggable subtree, into the app root.
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
        className="window helper-view"
        role="dialog"
        aria-label={label}
        style={{
          width: 'min(760px, 92vw)',
          height: 'min(640px, 86vh)',
          display: 'flex',
          flexDirection: 'column',
          boxShadow: 'var(--shadow), var(--border-window-outer), var(--border-window-inner)'
        }}
      >
        <div className="title-bar">
          <div className="title-bar-text" style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
            <Icon name={provider} size={13} />
            {title}
          </div>
          <div className="title-bar-controls">
            <button aria-label="Close" onClick={onClose} />
          </div>
        </div>
        <div
          className="window-body"
          style={{ flex: '1 1 auto', display: 'flex', flexDirection: 'column', minHeight: 0, gap: 6 }}
        >
          {header}
          <div style={{ fontSize: 'var(--fs-1)', color: 'var(--dim)' }} data-testid="helper-view-status">
            {status}
            {t.leftOut > 0 && ` · ${t.leftOut} older message${t.leftOut > 1 ? 's' : ''} not shown`}
            {t.passedOver && ' · earlier parts and very long entries not read'}
          </div>
          <div
            ref={list}
            onScroll={e => {
              const el = e.currentTarget;
              atEnd.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
            }}
            style={{
              flex: '1 1 auto',
              overflow: 'auto',
              background: 'var(--card)',
              border: '1px solid var(--line)',
              padding: 8,
              display: 'grid',
              alignContent: 'start',
              gap: 10
            }}
          >
            {t.messages.length === 0 && !t.loading && (
              <div style={{ color: 'var(--faint)', fontSize: 'var(--fs-3)' }}>Nothing in its conversation yet.</div>
            )}
            {t.messages.map((m, i) => (
              <div key={i} data-role={m.role} style={{ display: 'grid', gap: 2 }}>
                <div
                  style={{
                    fontSize: 'var(--fs-1)',
                    fontWeight: 700,
                    color: m.role === 'user' ? 'var(--accent)' : 'var(--dim)'
                  }}
                >
                  {ROLE[m.role] ?? m.role}
                </div>
                <div
                  style={{
                    whiteSpace: 'pre-wrap',
                    overflowWrap: 'anywhere',
                    fontSize: 'var(--fs-3)',
                    fontFamily: m.role === 'assistant' ? undefined : 'ui-monospace, Consolas, monospace'
                  }}
                >
                  {m.text}
                </div>
              </div>
            ))}
          </div>
        </div>
      </div>
    </div>,
    host
  );
}
