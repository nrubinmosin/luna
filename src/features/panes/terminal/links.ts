import type { ILinkHandler } from '@xterm/xterm';
import { openLink } from '../../../ipc/browsers';
import { linkContext, useLinkChooser } from '../../browsers/browsers.store';

/**
 * The CLIs print OSC 8 links. xterm's stock handler asks `confirm()` first,
 * which the dialog plugin's init script turns into a `plugin:dialog|message`
 * call this window has no permission for, and then opens a window of the
 * WebView's own. The link goes to the user's browser instead (browsers.rs),
 * without the question; Shift+click asks which one.
 */
export const linkHandler = (chatId: string): ILinkHandler => ({
  activate: (e, uri) => {
    if (e.shiftKey) {
      useLinkChooser.getState().show(uri, chatId, e.clientX, e.clientY);
      return;
    }
    // `call` logs a failure, without the link: one may carry a sign-in code.
    void openLink(uri, linkContext(chatId)).catch(() => {});
  }
});
