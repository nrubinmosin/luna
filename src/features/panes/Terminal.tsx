import { useEffect, useRef, useState } from 'react';
import { attachClipboardImage, attachFiles, filesFrom } from '../../shared/lib/attach';
import { ACCENT, tint } from '../../shared/lib/format';
import type { Chat } from '../../shared/types';
import { useAccounts } from '../accounts/accounts.store';
import { acquire, release, TERM_FONT_FAMILY, TERM_FONT_SIZE } from './terminals';

/** A drag carrying real files, as opposed to a chat row being dropped into a pane. */
const hasFiles = (dt: DataTransfer | null) => !!dt && Array.from(dt.types).includes('Files');

/**
 * The pane's end of a terminal: where it sits, and what can be dropped on it.
 * The xterm itself belongs to `terminals.ts` and outlives this component, so
 * showing a chat again is a reattach rather than a fresh session handshake.
 */
export function Terminal({ chat, folderPath }: { chat: Chat; folderPath: string }) {
  const boxRef = useRef<HTMLDivElement>(null);
  // Resolved from the store rather than read once at mount: on a cold start the
  // account list arrives after the panes do, and spawning with an empty path
  // drops CLAUDE_CONFIG_DIR — the CLI then boots on the default config and
  // greets every pane with first-run onboarding.
  const accountsLoaded = useAccounts(s => s.loaded);
  const accountPath = useAccounts(s => s.accounts.find(a => a.name === chat.account)?.path ?? null);
  const [dropping, setDropping] = useState(false);
  const [attaching, setAttaching] = useState(0);

  // Clipboard and drag payloads never carry a usable filesystem path in a
  // webview, so copy the bytes into the app's media store and type the
  // resulting absolute path into the prompt.
  const take = (files: File[]) => {
    if (!files.length) return;
    setAttaching(n => n + files.length);
    void attachFiles(chat.id, files).finally(() => setAttaching(n => Math.max(0, n - files.length)));
  };

  /** Ctrl+V with an image on the clipboard; falls through to normal paste otherwise. */
  const takeClipboardImage = () => {
    setAttaching(n => n + 1);
    void attachClipboardImage(chat.id).finally(() => setAttaching(n => Math.max(0, n - 1)));
  };

  useEffect(() => {
    const box = boxRef.current;
    if (!box || !accountsLoaded) return;
    const term = acquire({ chat, folderPath, accountPath });
    // An appendChild moves the element, so a terminal that was showing in
    // another pane simply arrives here — no second xterm for one session.
    box.appendChild(term.host);
    term.wake();
    return () => release(chat.id, box);
    // Session identity is the chat id; the rest is captured at spawn time.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [chat.id, accountsLoaded, accountPath]);

  return (
    <div
      onPasteCapture={e => {
        const files = filesFrom(e.clipboardData);
        if (files.length) {
          e.preventDefault();
          e.stopPropagation();
          take(files);
          return;
        }
        // WebView2 does not always expose a clipboard image to web content, and
        // xterm would forward an empty paste. When there is no text either, go
        // ask the OS whether it is holding an image.
        if (!e.clipboardData?.getData('text')) {
          e.preventDefault();
          e.stopPropagation();
          takeClipboardImage();
        }
      }}
      onDragOver={e => {
        if (!hasFiles(e.dataTransfer)) return; // a chat drag — let the pane handle it
        e.preventDefault();
        e.stopPropagation();
        e.dataTransfer.dropEffect = 'copy';
        setDropping(true);
      }}
      onDragLeave={e => {
        if (e.currentTarget.contains(e.relatedTarget as Node | null)) return;
        setDropping(false);
      }}
      onDrop={e => {
        if (!hasFiles(e.dataTransfer)) return;
        e.preventDefault();
        e.stopPropagation();
        setDropping(false);
        take(filesFrom(e.dataTransfer));
      }}
      style={{
        flex: 1, minHeight: 0, background: 'var(--term)', padding: '12px 14px',
        fontFamily: TERM_FONT_FAMILY, fontSize: TERM_FONT_SIZE,
        color: 'var(--dim)', overflow: 'hidden', display: 'flex', flexDirection: 'column',
        position: 'relative'
      }}
    >
      <div ref={boxRef} style={{ flex: 1, minHeight: 0, display: 'flex' }} />

      {dropping && (
        <div
          style={{
            position: 'absolute', inset: 6, borderRadius: 9, pointerEvents: 'none',
            border: `1.5px dashed ${ACCENT}`, background: tint(14, 'transparent'),
            display: 'grid', placeItems: 'center', fontSize: 'var(--fs-5)', fontWeight: 590, color: ACCENT,
            fontFamily: 'var(--sans-serif)'
          }}
        >
          Drop to attach
        </div>
      )}
      {attaching > 0 && (
        <div
          style={{
            position: 'absolute', right: 12, bottom: 10, pointerEvents: 'none',
            padding: '3px 9px', borderRadius: 7, background: 'var(--chip)',
            fontSize: 'var(--fs-3)', color: 'var(--dim)', fontFamily: 'var(--sans-serif)'
          }}
        >
          attaching {attaching} file{attaching > 1 ? 's' : ''}…
        </div>
      )}
    </div>
  );
}
