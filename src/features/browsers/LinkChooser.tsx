import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { openLink } from '../../ipc/browsers';
import { useDialogHost } from '../../shared/ui/dialogHost';
import { useChats } from '../chats/chats.store';
import { choices, linkContext, useBrowsers, useLinkChooser } from './browsers.store';

/**
 * What a Shift+click on a terminal's link brings up: every browser to open
 * it in, the chat's remembered one ticked, a box to remember the pick for
 * the chat, and the link to copy instead.
 */
export function LinkChooser() {
  const at = useLinkChooser(s => s.at);
  const data = useBrowsers(s => s.data);
  const remembered = useChats(s => (at ? (s.findChat(at.chatId)?.browser ?? null) : null));
  const [remember, setRemember] = useState(false);
  const host = useDialogHost();
  // Placed by its own size, measured, at the pointer and clamped to the
  // window; past the window's height it scrolls.
  const box = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  const count = choices(data).length;
  useLayoutEffect(() => {
    if (!at || !box.current) return;
    const w = box.current.offsetWidth;
    const h = box.current.offsetHeight;
    setPos({
      left: Math.max(8, Math.min(at.x, window.innerWidth - w - 8)),
      top: Math.max(8, Math.min(at.y, window.innerHeight - h - 8))
    });
  }, [at, host, count, remembered]);

  useEffect(() => {
    if (!at) return;
    // eslint-disable-next-line react-hooks/set-state-in-effect -- each opening (a new `at`) starts with the box unticked
    setRemember(false);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') useLinkChooser.getState().hide();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [at]);

  if (!at || !host) return null;
  const hide = () => useLinkChooser.getState().hide();
  const pick = (id: string) => {
    if (remember) useChats.getState().setBrowser(at.chatId, id);
    // `call` logs a failure, without the link: one may carry a sign-in code.
    void openLink(at.url, { ...linkContext(at.chatId), browser: id }).catch(() => {});
    hide();
  };
  const row = { padding: '3px 10px', cursor: 'default', whiteSpace: 'nowrap' as const };

  return createPortal(
    <div onClick={hide} onContextMenu={hide} style={{ position: 'fixed', inset: 0, zIndex: 80 }}>
      <div
        ref={box}
        onClick={e => e.stopPropagation()}
        className="window"
        style={{
          position: 'fixed',
          left: pos?.left ?? -9999,
          top: pos?.top ?? 0,
          minWidth: 240,
          maxWidth: 360,
          maxHeight: 'calc(100vh - 16px)',
          overflowY: 'auto',
          padding: '4px 0',
          boxShadow: 'var(--shadow)'
        }}
      >
        <div
          style={{
            ...row,
            color: 'var(--faint)',
            fontSize: 'var(--fs-1)',
            overflow: 'hidden',
            textOverflow: 'ellipsis'
          }}
          title={at.url}
        >
          {at.url}
        </div>
        {choices(data).map(b => (
          <div key={b.id} className="hover-bg" style={row} title={b.command} onClick={() => pick(b.id)}>
            {b.id === remembered ? '✓ ' : ''}
            {b.name}
          </div>
        ))}
        <div className="field-row" style={{ ...row, borderTop: '1px solid var(--line)', marginTop: 4, paddingTop: 5 }}>
          <input type="checkbox" id="link-remember" checked={remember} onChange={e => setRemember(e.target.checked)} />
          <label htmlFor="link-remember" style={{ cursor: 'default' }}>
            Remember for this chat
          </label>
        </div>
        {remembered && (
          <div
            className="hover-bg"
            style={row}
            onClick={() => {
              useChats.getState().setBrowser(at.chatId, null);
              hide();
            }}
          >
            Forget this chat’s browser
          </div>
        )}
        <div
          className="hover-bg"
          style={row}
          onClick={() => {
            void navigator.clipboard.writeText(at.url).catch(() => {});
            hide();
          }}
        >
          Copy link
        </div>
      </div>
    </div>,
    host
  );
}
