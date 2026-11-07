import type { Terminal as XTerm } from '@xterm/xterm';
import { logWarn } from '../../../shared/lib/log';

/**
 * Ctrl+C over a selection copies it, as in any Windows terminal, instead of
 * interrupting the CLI; with nothing selected it is ^C as ever. By physical
 * key, so a Cyrillic layout gets it too. Nothing is copied on selecting alone.
 *
 * Ctrl+V is the browser's: xterm would send it to the CLI as ^V and cancel the
 * key, so no paste would ever reach the pane — neither xterm's own, which types
 * the text, nor Terminal.tsx's, which attaches files and screenshots.
 */
export function attachClipboardKeys(term: XTerm) {
  term.attachCustomKeyEventHandler(e => {
    if (!e.ctrlKey || e.shiftKey || e.altKey || e.metaKey) return true;
    if (e.code === 'KeyV') return false;
    if (e.type !== 'keydown' || e.code !== 'KeyC' || !term.hasSelection()) return true;
    const text = term.getSelection();
    term.clearSelection();
    navigator.clipboard.writeText(text).catch(err => logWarn('terminal', `copy failed: ${String(err)}`));
    e.preventDefault();
    return false;
  });
}
