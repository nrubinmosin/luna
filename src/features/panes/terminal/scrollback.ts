import type { Terminal as XTerm } from '@xterm/xterm';
import { sessionModes } from '../../../ipc/sessions';

/**
 * The CLI spends most of its life fullscreen, i.e. on the alternate screen,
 * where there is no history and a scrollback buffer only bought a viewport that
 * could sit parked above the prompt. But it does leave that screen — opening a
 * shell's details prints straight onto the normal one — and with no buffer at
 * all everything scrolled past was gone for good, including the CLI's own UI.
 * Keep a buffer for those stretches; the alternate screen is pinned to the
 * bottom (`pinToBottom`).
 */
export const SCROLLBACK = 1000;

/**
 * Both screens have to be pinned to the bottom the moment they are switched
 * to. The alternate screen is a single painted frame that reads as a broken one
 * when parked above the prompt — and the normal screen is worse: opening a
 * shell's details drops the CLI onto it at whatever scroll position the normal
 * buffer was left at, which is the top of the conversation. The details then
 * print below the viewport, out of sight, and the CLI's own footer with them.
 * xterm follows new output on its own once the viewport is at the bottom, so
 * landing there is the whole fix.
 */
export function pinToBottom(term: XTerm): { dispose(): void } {
  const pinBottom = () => term.scrollToBottom();
  pinBottom();
  return term.buffer.onBufferChange(pinBottom);
}

/**
 * Reattaching to a live session. The replayed bytes are a tail: the escape
 * that entered the alternate screen sits at the very start of the session and
 * has long since been trimmed by the scrollback cap, so replaying leaves the
 * terminal on the normal screen with the CLI's UI scattered through its
 * history. Wipe that and ask for a real repaint.
 *
 * The two steps have to be ordered explicitly. term.write() only queues the
 * bytes — xterm parses them on its own schedule — while reset() runs there and
 * then, so writing and resetting back to back usually reset an empty terminal
 * and then painted the stale tail over the top of it, and the repaint went out
 * before any of it landed. That is what left panes showing a bare prompt box
 * with the conversation missing until the next keystroke. The callback is the
 * only ordering guarantee xterm offers.
 *
 * The reset also clears the modes the CLI switched at start — a hidden cursor
 * above all, and bracketed paste — and a repaint does not switch them again,
 * so xterm's own cursor showed up wherever the CLI last wrote and wandered the
 * screen from there. The core keeps them for the whole session; they go back
 * in before the repaint is asked for.
 */
export function replayTail(
  term: XTerm,
  chatId: string,
  backlog: string,
  disposed: () => boolean,
  nudgeRepaint: () => void
) {
  term.write(backlog, () => {
    if (disposed()) return;
    term.reset();
    void sessionModes(chatId)
      .catch(() => '')
      .then(modes => {
        if (disposed()) return;
        if (modes) term.write(modes);
        nudgeRepaint();
      });
  });
}
