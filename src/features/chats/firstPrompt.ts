import { writeSession } from '../../ipc/commands';
import { onPtyOutput } from '../../ipc/events';
import { logWarn } from '../../shared/lib/log';
import { useChats } from './chats.store';

// How long the CLI's output has to stay silent before its input box is assumed
// drawn and ready, and how long to keep waiting for that silence before giving
// up. The CLI can spend seconds on its welcome screen and update check.
const QUIET_MS = 700;
const WAIT_MS = 30_000;
// Gap between the message and the Enter that submits it, long enough that the
// CLI cannot mistake the two for one pasted block.
const ENTER_MS = 300;

const sleep = (ms: number) => new Promise(r => setTimeout(r, ms));

/**
 * Reads the chat's queued first message and clears it in the same tick, so two
 * callers racing on the same chat cannot both send it.
 */
const takePendingPrompt = (chatId: string): string | null => {
  const state = useChats.getState();
  const chat = state.folders.flatMap(f => f.chats).find(c => c.id === chatId);
  const prompt = chat?.pendingPrompt?.trim();
  if (!prompt) return null;
  state.clearPendingPrompt(chatId);
  return prompt;
};

/**
 * Types a chat's queued first message into its session and submits it.
 *
 * The CLI throws away anything written before it has entered raw mode and drawn
 * its input box, and how long that takes varies with the machine and with what
 * the CLI does on startup, so this waits for its output to go quiet rather than
 * guessing a delay. The message is taken from the store at the moment it is
 * sent: if the wait ends with nothing to show for it, it stays queued for
 * whoever tries next.
 *
 * @param running the session was already up, so its startup has long passed and
 *   the silence to wait for starts now rather than at its first byte.
 */
export async function sendFirstPrompt(chatId: string, running = false) {
  let lastOutputAt = running ? Date.now() : 0;
  const unlisten = await onPtyOutput(p => {
    if (p.id === chatId) lastOutputAt = Date.now();
  });

  try {
    const deadline = Date.now() + WAIT_MS;
    while (!(lastOutputAt > 0 && Date.now() - lastOutputAt >= QUIET_MS)) {
      if (Date.now() >= deadline) {
        // Out of time with the session never having said a word: it did not
        // come up, and typing into it would only lose the message.
        if (lastOutputAt === 0) {
          logWarn('chats', `session ${chatId} was silent for ${WAIT_MS}ms; first message still queued`);
          return;
        }
        break;
      }
      await sleep(150);
    }

    const first = takePendingPrompt(chatId);
    if (!first) return;
    // The carriage return goes in its own write, a beat later. The CLI reads
    // text arriving in one chunk with a newline in it as a paste and inserts a
    // line break instead of submitting — which left the message sitting in the
    // input box, typed but never sent.
    await writeSession(chatId, first);
    await sleep(ENTER_MS);
    await writeSession(chatId, '\r');
  } finally {
    unlisten();
  }
}
