const tauriAvailable = '__TAURI_INTERNALS__' in window;

/** A desktop notification — the app may be hidden in the tray. */
export async function notify(title: string, body: string): Promise<void> {
  if (!tauriAvailable) return;
  try {
    const { isPermissionGranted, requestPermission, sendNotification } = await import(
      '@tauri-apps/plugin-notification'
    );
    let granted = await isPermissionGranted();
    if (!granted) granted = (await requestPermission()) === 'granted';
    if (granted) sendNotification({ title, body });
  } catch {
    // notifications are best-effort
  }
}

// Toast when a session flips to "waiting".
export const notifyWaiting = (chatName: string, who: string): Promise<void> =>
  notify(`${who} is waiting`, `“${chatName}” needs your input`);
