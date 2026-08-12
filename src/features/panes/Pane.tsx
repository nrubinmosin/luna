import type { CSSProperties } from 'react';
import { STATUS } from '../../shared/ui/status';
import { ACCENT, limitColor, tail2, tint } from '../../shared/lib/format';
import { useChats } from '../chats/chats.store';
import { currentSlots, usePanes } from './panes.store';
import { Terminal } from './Terminal';

export function Pane({ index }: { index: number }) {
  const chatId = usePanes(s => currentSlots(s)[index]);
  const over = usePanes(s => s.over === index);
  const spotted = usePanes(s => !!s.spot && s.spot === currentSlots(s)[index]);
  const layout = usePanes(s => s.layout);
  const { setOver, dropChat, closePane, setDragPane, swapPanes } = usePanes.getState();
  const beingDragged = usePanes(s => s.dragPane === index);
  const chat = useChats(s => s.findChat(chatId));
  const folder = useChats(s => (chatId ? s.folderOf(chatId) : null));

  const st = chat ? STATUS[chat.status] : null;
  const t = folder ? tail2(folder.path) : null;
  const ctx = chat ? Math.round((chat.context ?? 0) * 100) : 0;
  const tokens = chat?.contextTokens ?? null;
  const window = chat?.contextWindow ?? null;
  const fmtTokens = (n: number) => (n >= 1e6 ? `${(n / 1e6).toFixed(2)}M` : `${Math.round(n / 1000)}k`);
  const ctxTitle =
    tokens != null && window != null
      ? `Context: ${fmtTokens(tokens)} of ${fmtTokens(window)} tokens (${ctx}%)` +
        '\nInput plus cache on the last turn. The window comes from the model the ' +
        'transcript recorded, not from the chat setting.'
      : 'Context window usage — waiting for the first turn';

  const chip: CSSProperties = {
    fontSize: 11.5, color: 'var(--dim)', background: 'var(--chip)',
    padding: '2px 6px', borderRadius: 5, whiteSpace: 'nowrap'
  };

  return (
    <div
      onDragOver={e => {
        // File drags belong to the terminal's attach handler, not pane placement.
        if (Array.from(e.dataTransfer.types).includes('Files')) return;
        e.preventDefault();
        // Dropping a pane onto itself is a no-op; don't advertise it as a target.
        if (usePanes.getState().dragPane === index) return;
        setOver(index);
      }}
      onDragLeave={() => usePanes.setState(s => (s.over === index ? { over: -1 } : s))}
      onDrop={e => {
        if (Array.from(e.dataTransfer.types).includes('Files')) return;
        e.preventDefault();
        const { dragPane, drag } = usePanes.getState();
        if (dragPane != null) {
          swapPanes(dragPane, index);
          return;
        }
        const id = drag || e.dataTransfer.getData('text/plain');
        if (id) dropChat(index, id);
        else setOver(-1);
      }}
      style={{
        flex: 1, minWidth: 0, minHeight: 0, display: 'flex', flexDirection: 'column',
        background: 'var(--bg)',
        border: `1px solid ${over || spotted ? ACCENT : 'var(--line)'}`,
        borderRadius: 11, overflow: 'hidden', transition: 'box-shadow .12s, border-color .12s',
        opacity: beingDragged ? 0.4 : 1,
        boxShadow: over
          ? `0 0 0 3px ${tint(28, 'transparent')}`
          : spotted
            ? `0 0 0 3px ${tint(20, 'transparent')}`
            : '0 1px 2px oklch(.4 .04 160 / .07)'
      }}
    >
      {chat && folder ? (
        <>
          <div
            draggable
            onDragStart={e => {
              e.dataTransfer.effectAllowed = 'move';
              // Something must be set or Firefox/WebView2 cancels the drag; the
              // pane index travels in the store, where the drop handler reads it.
              e.dataTransfer.setData('text/plain', '');
              setDragPane(index);
            }}
            onDragEnd={() => {
              setDragPane(null);
              setOver(-1);
            }}
            title="Drag to swap this pane with another"
            style={{
              flex: 'none', height: 38, display: 'flex', alignItems: 'center', gap: 8, padding: '0 10px',
              cursor: 'grab',
              background: chat.status === 'working' ? tint(11, 'var(--panel)') : 'var(--panel)',
              borderBottom: '1px solid var(--line)'
            }}
          >
            <span
              title={st!.label}
              style={{ width: 8, height: 8, borderRadius: '50%', flex: 'none', background: st!.color, animation: st!.anim }}
            />
            <span style={{ flex: '1 1 auto', minWidth: 60, fontSize: 13, fontWeight: 600, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
              {chat.name}
            </span>
            <span
              onClick={() => closePane(index)}
              className="hover-bg"
              style={{ width: 18, height: 18, borderRadius: 5, display: 'grid', placeItems: 'center', fontSize: 12, color: 'var(--dim)', cursor: 'default', flex: 'none' }}
            >
              ✕
            </span>
          </div>
          <div style={{ flex: 'none', display: 'flex', flexWrap: 'wrap', alignItems: 'center', gap: 5, padding: '7px 10px 8px', background: 'var(--panel)', borderBottom: '1px solid var(--line)' }}>
            <span style={{ fontSize: 11, color: 'var(--faint)', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis', maxWidth: '100%' }}>
              {t!.parent} / {t!.leaf}
            </span>
            <span
              title={`Running on Claude account "${chat.account}"`}
              style={{ ...chip, display: 'flex', alignItems: 'center', gap: 4, maxWidth: 160 }}
            >
              <span style={{ width: 5, height: 5, borderRadius: '50%', flex: 'none', background: 'var(--accent)' }} />
              <span style={{ overflow: 'hidden', textOverflow: 'ellipsis' }}>{chat.account}</span>
            </span>
            <span style={chip}>{chat.model}</span>
            <span style={chip}>effort: {chat.effort}</span>
            <span style={chip}>{chat.perm}</span>
            {chat.worktree && <span style={chip}>worktree</span>}
            <span
              title={ctxTitle}
              style={{ display: 'flex', alignItems: 'center', gap: 5, padding: '2px 6px 2px 5px', borderRadius: 5, background: 'var(--chip)', whiteSpace: 'nowrap' }}
            >
              <span style={{ width: 26, height: 5, borderRadius: 3, background: 'var(--track)', overflow: 'hidden', flex: 'none' }}>
                <span style={{ display: 'block', height: '100%', borderRadius: 2, width: `${ctx}%`, background: limitColor(chat.context ?? 0) }} />
              </span>
              <span style={{ fontSize: 11.5, color: 'var(--fg)', fontVariantNumeric: 'tabular-nums' }}>
                {tokens != null ? fmtTokens(tokens) : '—'}
              </span>
              <span style={{ fontSize: 11.5, color: limitColor(chat.context ?? 0), fontVariantNumeric: 'tabular-nums' }}>
                {window != null ? `${ctx}%` : 'ctx'}
              </span>
            </span>
          </div>
          <Terminal chat={chat} folderPath={folder.path} />
        </>
      ) : (
        <div style={{ flex: 1, display: 'grid', placeItems: 'center', gap: 6, alignContent: 'center', color: 'var(--faint)' }}>
          <div style={{ width: 40, height: 30, border: '1.5px dashed var(--line)', borderRadius: 6 }} />
          <div style={{ fontSize: 13, whiteSpace: 'nowrap' }}>Drag a chat here</div>
          <div style={{ fontSize: 11.5, opacity: 0.75, whiteSpace: 'nowrap' }}>
            pane {index + 1} of {layout}
          </div>
        </div>
      )}
    </div>
  );
}
