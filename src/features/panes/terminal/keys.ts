import type { Terminal as XTerm } from '@xterm/xterm';
import { logWarn } from '../../../shared/lib/log';

/**
 * Alt+arrow as xterm sent it up to 5: as Ctrl+arrow, so Alt+Left and
 * Alt+Right move a word in the CLI's input (on macOS ESC b and ESC f, the
 * same moves there). xterm 6 sends Alt's own sequences and leaves this to the
 * embedder. By key, with no other modifier held.
 */
const ALT_ARROWS = new Map<string, { other: string; mac?: string }>([
  ['ArrowLeft', { other: '\x1b[1;5D', mac: '\x1bb' }],
  ['ArrowRight', { other: '\x1b[1;5C', mac: '\x1bf' }],
  ['ArrowUp', { other: '\x1b[1;5A' }],
  ['ArrowDown', { other: '\x1b[1;5B' }]
]);

const onMac = () => navigator.platform.startsWith('Mac');

/**
 * The keys a chat's terminal handles itself, in the one handler xterm takes.
 *
 * Ctrl+C over a selection copies it, as in any Windows terminal, instead of
 * interrupting the CLI; with nothing selected it is ^C as ever. By physical
 * key, so a Cyrillic layout gets it too. Nothing is copied on selecting alone.
 *
 * Ctrl+V is the browser's: xterm would send it to the CLI as ^V and cancel the
 * key, so no paste would ever reach the pane — neither xterm's own, which types
 * the text, nor Terminal.tsx's, which attaches files and screenshots.
 *
 * Alt+arrow sends what it did before xterm 6 (`ALT_ARROWS`).
 */
export function attachKeys(term: XTerm) {
  term.attachCustomKeyEventHandler(e => {
    const seq = ALT_ARROWS.get(e.key);
    if (seq && e.altKey && !e.ctrlKey && !e.shiftKey && !e.metaKey) {
      if (e.type !== 'keydown') return true;
      const send = onMac() ? seq.mac : seq.other;
      // Left to xterm where the old mapping changed nothing.
      if (!send) return true;
      term.input(send);
      // Alt+Left is the browser's Back as well.
      e.preventDefault();
      return false;
    }
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
