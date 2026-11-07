import { useChats } from '../../chats/chats.store';

/**
 * Keystrokes only. xterm's `onData` also carries what the terminal itself
 * answers: the cursor-position reply the CLI asks for at startup, focus in/out,
 * and an SGR mouse report for every pointer move across a pane. Dropping just
 * the escape byte left their parameters behind as text, which is how a chat
 * came to be titled `[1;1R[O[O[<35;1;14M…` — so take the whole sequence out.
 */
// eslint-disable-next-line no-control-regex -- matching the escape byte is the point
const typedOnly = (d: string) => d.replace(/\x1b(?:\[[\d;<>?]*[ -/]*[@-~]|O.|.)?/g, '');

/**
 * The title is the first thing typed into the chat, taken as it is typed.
 * The registry route — CLI writes its entry, the watcher polls it a few
 * seconds later — leaves a new chat sitting in the list as "chat 12" while
 * everything around it is named after its prompt, which is a very easy row
 * to lose track of. Reading the keystrokes is a guess (an edited line keeps
 * the characters that were rubbed out), but the watcher replaces it with
 * the transcript's own version as soon as there is one. Only a chat still
 * on its placeholder takes it: this terminal is rebuilt whenever its pane
 * comes back, and the next line typed used to rename a long-named chat.
 *
 * Returns what to hand each piece of the terminal's `onData`.
 */
export function titleFromTyping(chatId: string): (data: string) => void {
  let typed = '';
  let titled = false;
  return d => {
    if (titled) return;
    const clean = typedOnly(d);
    const enter = clean.indexOf('\r');
    typed += enter < 0 ? clean : clean.slice(0, enter);
    if (typed.length > 400) typed = typed.slice(-400);
    if (enter >= 0) {
      // eslint-disable-next-line no-control-regex -- control characters are what it strips
      const line = typed.replace(/[\x00-\x1f\x7f]/g, '').trim();
      typed = '';
      // A slash command is the CLI's business, not a title.
      if (line.length > 1 && !line.startsWith('/')) {
        titled = true;
        if (useChats.getState().findChat(chatId)?.nameSource === undefined) {
          useChats.getState().setName(chatId, line.slice(0, 80), 'prompt');
        }
      }
    }
  };
}
