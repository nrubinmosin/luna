import { useNotifyPrefs } from './notifyPrefs.store';

const tauriAvailable = '__TAURI_INTERNALS__' in window;

/**
 * The system's own notification sound, by the name each platform knows it
 * by. Without one, Windows shows the toast silent.
 */
const systemSound = (): string => {
  const ua = navigator.userAgent;
  if (ua.includes('Windows')) return 'Default';
  if (ua.includes('Mac')) return 'Ping';
  return 'message-new-instant';
};

/**
 * A desktop notification — the app may be hidden in the tray. One about a
 * chat carries its id, and a click on it opens that chat (lib.rs).
 */
export async function notify(title: string, body: string, chatId?: string | null): Promise<void> {
  if (!tauriAvailable) return;
  try {
    const { isPermissionGranted, requestPermission, sendNotification } = await import(
      '@tauri-apps/plugin-notification'
    );
    let granted = await isPermissionGranted();
    if (!granted) granted = (await requestPermission()) === 'granted';
    if (granted) {
      sendNotification({
        title,
        body,
        sound: useNotifyPrefs.getState().sound ? systemSound() : undefined,
        extra: chatId ? { chatId } : undefined
      });
    }
  } catch {
    // notifications are best-effort
  }
}

// Toast when a session flips to "waiting".
export const notifyWaiting = (chatId: string, chatName: string, who: string): Promise<void> =>
  notify(`${who} is waiting`, `“${chatName}” needs your input`, chatId);
