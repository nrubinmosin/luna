import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import type { CSSProperties } from 'react';
import { createPortal } from 'react-dom';
import { useDialogHost } from '../../shared/ui/dialogHost';
import { Icon } from '../../shared/ui/icons';
import { StatusDot } from '../../shared/ui/StatusDot';
import { CHAT_COLORS, chatColorTheme } from '../../shared/ui/chatColors';
import { ACCENT, limitColor, shortSessionId, tail2, tint } from '../../shared/lib/format';
import { useChats } from '../chats/chats.store';
import { DeleteChatDialog } from '../chats/DeleteChatDialog';
import { claude, codex } from '../providers';
import { PROVIDER_LABEL, type Chat } from '../../shared/types';
import { currentLayout, currentSlots, usePanes } from './panes.store';
import { Terminal } from './Terminal';

/**
 * The chat's colour, picked from a popover under the swatch button. Portalled
 * and placed by the button's own rect, clamped to the window: drawn inside
 * the pane it was cut off by the pane's edge, or ran off the window's.
 */
function ColorPicker({ chat, anchor, onClose }: { chat: Chat; anchor: HTMLElement; onClose: () => void }) {
  const host = useDialogHost();
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);

  useLayoutEffect(() => {
    const r = anchor.getBoundingClientRect();
    const w = ref.current?.offsetWidth ?? 180;
    const h = ref.current?.offsetHeight ?? 70;
    const left = Math.min(Math.max(8, r.left - 4), window.innerWidth - w - 8);
    const below = r.bottom + 8;
    setPos({ left, top: below + h > window.innerHeight - 8 ? Math.max(8, r.top - h - 8) : below });
  }, [anchor, host]);

  // Any click the popover didn't swallow dismisses it, and so does Escape.
  useEffect(() => {
    const close = () => onClose();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    document.addEventListener('click', close);
    window.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('click', close);
      window.removeEventListener('keydown', onKey);
    };
  }, [onClose]);

  if (!host) return null;
  const set = (key: string | null) => {
    useChats.getState().setColor(chat.id, key);
    onClose();
  };
  return createPortal(
    <div
      ref={ref}
      className="popover"
      onClick={e => e.stopPropagation()}
      style={{
        left: pos?.left ?? -9999, top: pos?.top ?? 0,
        display: 'grid', gridTemplateColumns: 'repeat(6, 22px)', gap: 6
      }}
    >
      <span
        className="swatch"
        data-on={!chat.color ? 'yes' : 'no'}
        onClick={() => set(null)}
        title="No colour — stock Luna blue"
        style={{ background: 'var(--chip)' }}
      >
        <Icon name="close" size={12} />
      </span>
      {CHAT_COLORS.map(c => (
        <span
          key={c.key}
          className="swatch"
          data-on={chat.color === c.key ? 'yes' : 'no'}
          onClick={() => set(c.key)}
          title={c.label}
          style={{ background: chatColorTheme(c.key)!.swatch }}
        />
      ))}
    </div>,
    host
  );
}

/**
 * `soloChat` renders the pane on the peek sheet for a chat with no seat on the
 * board: the chat comes from the prop instead of a slot, so every control that
 * would touch the board (drag, drop, close) turns into a plain "back to the
 * grid".
 */
export function Pane({ index = -1, soloChat }: { index?: number; soloChat?: string }) {
  const chatId = usePanes(s => soloChat ?? currentSlots(s)[index]);
  const over = usePanes(s => !soloChat && s.over === index);
  const spotted = usePanes(s => !!s.spot && s.spot === (soloChat ?? currentSlots(s)[index]));
  const layout = usePanes(currentLayout);
  const { setOver, dropChat, closePane, setDragPane, swapPanes, setPeek, setPeekChat, setActivePane } =
    usePanes.getState();
  const peeked = usePanes(s => (soloChat ? true : s.peek === index));
  // The pane you are working in wears its colour; the rest go pale, the way
  // XP drew every window but the active one. One pane, or a sheet, is always
  // the one you are in.
  const focused = usePanes(s => !!soloChat || peeked || currentLayout(s) === 1 || s.activePane === index);
  const beingDragged = usePanes(s => !soloChat && s.dragPane === index);
  const chat = useChats(s => s.findChat(chatId));
  const folder = useChats(s => (chatId ? s.folderOf(chatId) : null));
  const [confirming, setConfirming] = useState(false);
  const [editing, setEditing] = useState(false);
  const [picking, setPicking] = useState<HTMLElement | null>(null);
  const [copied, setCopied] = useState(false);
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
    if (!stale) return;
    if (soloChat) setPeekChat(null);
    else closePane(index);
  }, [stale, index, soloChat, closePane, setPeekChat]);

  // The slot can be handed a different chat while a rename or a delete prompt
  // is open; neither belongs to the newcomer.
  useEffect(() => {
    setEditing(false);
    setConfirming(false);
    setPicking(null);
  }, [chatId]);

  const t = folder ? tail2(folder.path) : null;
  const colorTheme = chatColorTheme(chat?.color);
  const ctx = chat ? Math.round((chat.context ?? 0) * 100) : 0;
  const tokens = chat?.contextTokens ?? null;
  const window = chat?.contextWindow ?? null;
  const fmtTokens = (n: number) => (n >= 1e6 ? `${(n / 1e6).toFixed(2)}M` : `${Math.round(n / 1000)}k`);
  const ctxTitle =
    tokens != null && window != null
      ? `Context: ${fmtTokens(tokens)} of ${fmtTokens(window)} tokens (${ctx}%)` +
        (chat?.provider === 'codex'
          ? '\nTokens in the window after the last turn, as Codex counts them; the window is the one it reported.'
          : '\nInput plus cache on the last turn. The window comes from the model the ' +
            'transcript recorded, not from the chat setting.')
      : 'Context window usage — waiting for the first turn';

  return (
    <div
      className="pane"
      data-focus={focused ? 'yes' : 'no'}
      data-waiting={chat?.status === 'waiting' ? 'yes' : 'no'}
      // Where a new chat lands when the board has no room: the pane you were
      // last working in, which is the one you just clicked into.
      onMouseDown={() => {
        if (!soloChat) setActivePane(index);
      }}
      onDragOver={e => {
        // A chat on the peek sheet is not a slot — nothing can be dropped in.
        if (soloChat) return;
        // File drags belong to the terminal's attach handler, not pane placement.
        if (Array.from(e.dataTransfer.types).includes('Files')) return;
        e.preventDefault();
        // Dropping a pane onto itself is a no-op; don't advertise it as a target.
        if (usePanes.getState().dragPane === index) return;
        setOver(index);
      }}
      onDragLeave={() => usePanes.setState(s => (s.over === index ? { over: -1 } : s))}
      onDrop={e => {
        if (soloChat) return;
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
            draggable={!soloChat && !editing && !picking}
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
            className="title-bar pane-bar"
            style={{ ['--c' as string]: colorTheme?.swatch ?? 'var(--luna-blue)' } as CSSProperties}
          >
            {/* Controls first: the hand reaches the left of a bar before its
                right, and the name reads as well from either end. */}
            <span className="title-bar-controls">
              <button
                data-glyph="swatch"
                aria-label="Chat colour"
                title="Chat colour — tints this title bar and the chat's row in the list"
                onClick={e => {
                  e.stopPropagation();
                  const at = e.currentTarget;
                  setPicking(p => (p ? null : at));
                }}
                style={{ ['--swatch' as string]: colorTheme?.swatch } as CSSProperties}
              />
              <button
                data-glyph="pencil"
                aria-label="Rename"
                title="Rename chat"
                onClick={e => {
                  e.stopPropagation();
                  setDraft(chat.name);
                  setEditing(true);
                }}
              />
              <button
                data-glyph="trash"
                aria-label="Delete chat"
                title="Delete chat — asks first"
                onClick={() => setConfirming(true)}
              />
              {(soloChat || layout > 1) && (
                <button
                  data-glyph={peeked ? 'restore' : 'max'}
                  aria-label={peeked ? 'Back to the grid' : 'Peek'}
                  title={peeked ? 'Back to the grid' : 'Peek — hold this pane up over the board, which stays as it is'}
                  onClick={() => (soloChat ? setPeekChat(null) : setPeek(peeked ? null : index))}
                />
              )}
              {/* A chat on the sheet holds no pane to leave — the restore
                  button already says "back". */}
              {!soloChat && (
                <button
                  data-glyph="min"
                  aria-label="Take off the board"
                  title="Take this chat off the board — it keeps running and stays in the list"
                  onClick={() => closePane(index)}
                />
              )}
            </span>
            {picking && <ColorPicker chat={chat} anchor={picking} onClose={() => setPicking(null)} />}

            <span className="pane-head">
              <StatusDot status={chat.status} />
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
                  onMouseDown={e => e.stopPropagation()}
                  type="text"
                  style={{ flex: '1 1 auto', minWidth: 60, height: 'calc(var(--ui) * 1.6)' }}
                />
              ) : (
                <span title={chat.name} className="title-bar-text">
                  {chat.name}
                </span>
              )}

              {!editing && (
                <span className="pane-chips">
                  <span className="pchip opt-3" title={folder.path}>{t!.leaf}</span>
                  <span className="pchip opt-1" title={`Running on ${PROVIDER_LABEL[chat.provider]} account "${chat.account}"`}>
                    <Icon name={chat.provider} size={12} />
                    {chat.account}
                  </span>
                  {chat.provider === 'codex' ? <codex.Meta chat={chat} /> : <claude.Meta chat={chat} />}
                  {chat.worktree && <span className="pchip opt-3" title="Isolated git worktree">worktree</span>}
                </span>
              )}

              <span title={ctxTitle} className="ctx-meter">
                <span className="track meter">
                  <i style={{ width: `${ctx}%`, ['--m' as string]: limitColor(chat.context ?? 0) } as CSSProperties} />
                </span>
                {window != null || (chat.context ?? 0) > 0 ? `${ctx}%` : '—'}
              </span>

              {/* How issue claims and logs name this session: the uuid's first
                  8 characters. A click copies exactly that. */}
              {chat.sessionId && (
                <span
                  onClick={e => {
                    e.stopPropagation();
                    navigator.clipboard
                      .writeText(shortSessionId(chat.sessionId!))
                      .then(() => {
                        setCopied(true);
                        setTimeout(() => setCopied(false), 1200);
                      })
                      .catch(() => {});
                  }}
                  title={`Session ${chat.sessionId}\nClick to copy ${shortSessionId(chat.sessionId)}`}
                  className="pane-sid"
                >
                  {copied ? 'copied' : shortSessionId(chat.sessionId)}
                </span>
              )}
            </span>
          </div>
          {confirming && <DeleteChatDialog chat={chat} onClose={() => setConfirming(false)} />}
          <div style={{ flex: 1, minHeight: 0, display: 'flex', flexDirection: 'column', position: 'relative' }}>
            <Terminal chat={chat} folderPath={folder.path} />
            {/* Keyed on the chat: a pane handed a new chat fades it in. */}
            <div key={chat.id} className="pane-cover" />
            {chat.status === 'waiting' && !focused && (
              <div className="balloon">
                <b>Waiting for you</b>
                Click into the pane to answer.
              </div>
            )}
          </div>
        </>
      ) : (
        <div style={{ flex: 1, display: 'grid', placeItems: 'center', gap: 6, alignContent: 'center', color: 'var(--faint)' }}>
          <div style={{ width: 40, height: 30, border: '1.5px dashed var(--line)', borderRadius: 3 }} />
          {/* With one pane a click in the list fills it; with more, which pane
              a chat goes to is a choice, and the drag is how it is made. */}
          <div style={{ fontSize: 'var(--fs-5)', whiteSpace: 'nowrap' }}>
            {layout === 1 ? 'Click a chat in the list' : 'Drag a chat here'}
          </div>
          <div style={{ fontSize: 'var(--fs-3)', opacity: 0.75, whiteSpace: 'nowrap' }}>
            pane {index + 1} of {layout}
          </div>
        </div>
      )}
    </div>
  );
}
