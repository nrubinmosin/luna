import { useEffect, useState } from 'react';
import type { CSSProperties } from 'react';
import { STATUS } from '../../shared/ui/status';
import { CHAT_COLORS, chatColorTheme } from '../../shared/ui/chatColors';
import { ACCENT, limitColor, tail2, tint } from '../../shared/lib/format';
import { useChats } from '../chats/chats.store';
import { DeleteChatDialog } from '../chats/DeleteChatDialog';
import { currentLayout, currentSlots, usePanes } from './panes.store';
import { Terminal } from './Terminal';

export function Pane({ index }: { index: number }) {
  const chatId = usePanes(s => currentSlots(s)[index]);
  const over = usePanes(s => s.over === index);
  const spotted = usePanes(s => !!s.spot && s.spot === currentSlots(s)[index]);
  const layout = usePanes(currentLayout);
  const { setOver, dropChat, closePane, setDragPane, swapPanes, setFocus } = usePanes.getState();
  const focused = usePanes(s => s.focus === index);
  const beingDragged = usePanes(s => s.dragPane === index);
  const chat = useChats(s => s.findChat(chatId));
  const folder = useChats(s => (chatId ? s.folderOf(chatId) : null));
  const [confirming, setConfirming] = useState(false);
  const [editing, setEditing] = useState(false);
  const [picking, setPicking] = useState(false);
  const [draft, setDraft] = useState('');

  const commitRename = () => {
    setEditing(false);
    const name = draft.trim();
    if (chat && name && name !== chat.name) useChats.getState().renameChat(chat.id, name);
  };

  // A slot pointing at a chat that no longer exists renders as an empty pane,
  // which is correct but silent — the board would keep the dead id forever.
  // Deleting a chat clears the boards itself; this covers the rest: a chat that
  // went away while the two persisted stores were out of step, which is what a
  // restore from a half-written state looks like.
  const stale = !!chatId && !chat;
  useEffect(() => {
    if (stale) closePane(index);
  }, [stale, index, closePane]);

  // The slot can be handed a different chat while a rename or a delete prompt
  // is open; neither belongs to the newcomer.
  useEffect(() => {
    setEditing(false);
    setConfirming(false);
    setPicking(false);
  }, [chatId]);

  // Any click that the popover didn't swallow dismisses it. Attached only
  // while it is open, and after the opening click's dispatch has finished.
  useEffect(() => {
    if (!picking) return;
    const close = () => setPicking(false);
    // `window` is shadowed here by the chat's context window.
    document.addEventListener('click', close);
    return () => document.removeEventListener('click', close);
  }, [picking]);

  const st = chat ? STATUS[chat.status] : null;
  const t = folder ? tail2(folder.path) : null;
  const colorTheme = chatColorTheme(chat?.color);
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
        // The top corners sit one pixel outside the title bar's own 8px curve,
        // which is what makes the frame hug it. At the 3px they used to share
        // with the bottom, the bar curved away from a frame that stayed square
        // and left a wedge of pane background showing in each corner.
        borderRadius: '9px 9px 3px 3px',
        overflow: 'hidden', transition: 'box-shadow .12s, border-color .12s',
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
            // Chromium hands every mouse press inside a draggable subtree to
            // the drag machinery, and selecting text in the rename input would
            // start a pane drag instead.
            draggable={!editing && !picking}
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
            // The bar itself toggles the same focus as its Maximize control;
            // children with a double-click of their own stop the bubble.
            onDoubleClick={() => {
              if (layout > 1) setFocus(focused ? null : index);
            }}
            className="title-bar"
            // Inline wins over xp.css's Luna-blue gradient and frame colours,
            // so an unset chat colour leaves the stock look untouched.
            style={{
              flex: 'none', gap: 5, cursor: 'grab',
              ...(colorTheme && {
                background: colorTheme.grad,
                borderTopColor: colorTheme.top,
                borderLeftColor: colorTheme.top,
                borderRightColor: colorTheme.right
              })
            }}
          >
            <span
              title={st!.label}
              style={{ width: 7, height: 7, borderRadius: '50%', flex: 'none', background: st!.color, animation: st!.anim }}
            />
            {editing ? (
              <input
                autoFocus
                value={draft}
                onChange={e => setDraft(e.target.value)}
                onBlur={commitRename}
                onKeyDown={e => {
                  if (e.key === 'Enter') commitRename();
                  if (e.key === 'Escape') setEditing(false);
                }}
                onClick={e => e.stopPropagation()}
                // Double-clicking a word to select it is not a focus toggle.
                onDoubleClick={e => e.stopPropagation()}
                type="text"
                style={{ flex: '1 1 auto', minWidth: 40, height: 'calc(var(--ui) * 1.4)' }}
              />
            ) : (
              <span
                onDoubleClick={e => {
                  e.stopPropagation();
                  setDraft(chat.name);
                  setEditing(true);
                }}
                title={`${chat.name}\nDouble-click to rename`}
                className="title-bar-text"
                // theme.css bakes a navy text shadow into .title-bar-text; on a
                // recoloured bar it reads as a blue smudge.
                style={{ flex: '1 1 auto', minWidth: 40, ...(colorTheme && { textShadow: `1px 1px ${colorTheme.shadow}` }) }}
              >
                {chat.name}
              </span>
            )}

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

            <span
              onDoubleClick={e => e.stopPropagation()}
              style={{ position: 'relative', flex: 'none', display: 'flex', alignItems: 'center' }}
            >
              <span
                onClick={e => {
                  e.stopPropagation();
                  setPicking(p => !p);
                }}
                title="Chat colour — tints this title bar and the chat's row in the list"
                style={{
                  width: 13, height: 13, borderRadius: 2, cursor: 'default',
                  border: '1px solid rgba(255,255,255,.85)',
                  background: colorTheme?.swatch ?? 'rgba(255,255,255,.35)'
                }}
              />
              {picking && (
                <span
                  onClick={e => e.stopPropagation()}
                  style={{
                    position: 'absolute', top: 'calc(100% + 6px)', right: -2, zIndex: 60,
                    display: 'flex', gap: 4, padding: 5, borderRadius: 3, background: 'var(--surface)',
                    boxShadow: 'var(--border-window-outer), var(--border-window-inner), var(--shadow)'
                  }}
                >
                  <span
                    onClick={() => {
                      useChats.getState().setColor(chat.id, null);
                      setPicking(false);
                    }}
                    title="No colour — stock Luna blue"
                    style={{
                      width: 15, height: 15, borderRadius: 2, cursor: 'default', flex: 'none',
                      display: 'grid', placeItems: 'center', fontSize: 10, color: 'var(--faint)',
                      outline: !chat.color ? '2px solid var(--fg)' : '1px solid var(--line)',
                      background: 'var(--chip)'
                    }}
                  >
                    ✕
                  </span>
                  {CHAT_COLORS.map(c => (
                    <span
                      key={c.key}
                      onClick={() => {
                        useChats.getState().setColor(chat.id, c.key);
                        setPicking(false);
                      }}
                      title={c.label}
                      style={{
                        width: 15, height: 15, borderRadius: 2, cursor: 'default', flex: 'none',
                        background: chatColorTheme(c.key)!.swatch,
                        outline: chat.color === c.key ? '2px solid var(--fg)' : '1px solid rgba(0,0,0,.25)'
                      }}
                    />
                  ))}
                </span>
              )}
            </span>

            <span className="title-bar-controls" onDoubleClick={e => e.stopPropagation()} style={{ flex: 'none' }}>
              <button
                aria-label="Delete"
                title="Delete chat — same confirmation as the list"
                onClick={() => setConfirming(true)}
              />
              {layout > 1 && (
                <button
                  aria-label={focused ? 'Restore' : 'Maximize'}
                  title={focused ? 'Back to the grid' : 'Focus this pane — the grid stays as it is'}
                  onClick={() => setFocus(focused ? null : index)}
                />
              )}
              <button
                aria-label="Close"
                title="Close this pane — the chat keeps running"
                onClick={() => closePane(index)}
              />
            </span>
          </div>
          {confirming && <DeleteChatDialog chat={chat} onClose={() => setConfirming(false)} />}
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
