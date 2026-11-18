import type { ILinkHandler } from '@xterm/xterm';
import { openLink } from '../../../ipc/links';

/**
 * The CLIs print OSC 8 links. xterm's stock handler asks `confirm()` first,
 * which the dialog plugin's init script turns into a `plugin:dialog|message`
 * call this window has no permission for, and then opens a window of the
 * WebView's own. The link goes to the system's handler instead, the user's
 * default browser or mail app (links.rs `open_link`), without the question.
 */
export const linkHandler: ILinkHandler = {
  activate: (_e, uri) => {
    // `call` logs a failure, without the link: one may carry a sign-in code.
    void openLink(uri).catch(() => {});
  }
};
