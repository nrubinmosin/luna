import type { CSSProperties } from 'react';
import { STATUS } from '../../shared/ui/status';
import { ACCENT, limitColor, tail2, tint } from '../../shared/lib/format';
import { useChats } from '../chats/chats.store';
import { currentLayout, currentSlots, usePanes } from './panes.store';
import { Terminal } from './Terminal';

export function Pane({ index }: { index: number }) {
  const chatId = usePanes(s => currentSlots(s)[index]);
  const over = usePanes(s => s.over === index);
  const spotted = usePanes(s => !!s.spot && s.spot === currentSlots(s)[index]);
  const layout = usePanes(currentLayout);
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
    fontSize: 'var(--fs-1)', color: '#0a1f5c', background: 'rgba(255,255,255,.82)',
    padding: '1px 5px', borderRadius: 2, whiteSpace: 'nowrap', flex: 'none'
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
        border: `1px solid ${over || spotted ? ACCENT : 'var(--window-frame)'}`,
        borderRadius: 3, overflow: 'hidden', transition: 'box-shadow .12s, border-color .12s',
        opacity: beingDragged ? 0.4 : 1,
        boxShadow: over
          ? `0 0 0 3px ${tint(28, 'transparent')}`
          : spotted
            ? `0 0 0 3px ${tint(20, 'transparent')}`
            : 'none'
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
            className="title-bar"
            style={{ flex: 'none', gap: 5, cursor: 'grab' }}
          >
            <span
              title={st!.label}
              style={{ width: 7, height: 7, borderRadius: '50%', flex: 'none', background: st!.color, animation: st!.anim }}
            />
            <span className="title-bar-text" style={{ flex: '1 1 auto', minWidth: 40 }}>
              {chat.name}
            </span>

            <span title={`${t!.parent} / ${t!.leaf}`} style={{ ...chip, maxWidth: 90, overflow: 'hidden', textOverflow: 'ellipsis' }}>
              {t!.leaf}
            </span>
            <span title={`Running on Claude account "${chat.account}"`} style={{ ...chip, maxWidth: 80, overflow: 'hidden', textOverflow: 'ellipsis' }}>
              {chat.account}
            </span>
            <span title="Model" style={chip}>{chat.model}</span>
            <span title={`Effort: ${chat.effort}`} style={chip}>{chat.effort}</span>
            <span title={`Permission mode: ${chat.perm}`} style={chip}>{chat.perm[0]}</span>
            {chat.worktree && <span title="Isolated git worktree" style={chip}>wt</span>}
            <span
              title={ctxTitle}
              style={{ ...chip, display: 'flex', alignItems: 'center', gap: 4 }}
            >
              <span style={{ width: 20, height: 4, borderRadius: 2, background: 'var(--track)', overflow: 'hidden', flex: 'none' }}>
                <span style={{ display: 'block', height: '100%', width: `${ctx}%`, background: limitColor(chat.context ?? 0) }} />
              </span>
              <span style={{ fontVariantNumeric: 'tabular-nums' }}>
                {window != null ? `${ctx}%` : '—'}
              </span>
            </span>

            <span className="title-bar-controls" style={{ flex: 'none' }}>
              <button
                aria-label="Close"
                title="Close this pane — the chat keeps running"
                onClick={() => closePane(index)}
              />
            </span>
          </div>
          <Terminal chat={chat} folderPath={folder.path} />
        </>
      ) : (
        <div style={{ flex: 1, display: 'grid', placeItems: 'center', gap: 6, alignContent: 'center', color: 'var(--faint)' }}>
          <div style={{ width: 40, height: 30, border: '1.5px dashed var(--line)', borderRadius: 3 }} />
          <div style={{ fontSize: 'var(--fs-5)', whiteSpace: 'nowrap' }}>Drag a chat here</div>
          <div style={{ fontSize: 'var(--fs-3)', opacity: 0.75, whiteSpace: 'nowrap' }}>
            pane {index + 1} of {layout}
          </div>
        </div>
      )}
    </div>
  );
}
