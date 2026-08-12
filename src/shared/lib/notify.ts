const tauriAvailable = '__TAURI_INTERNALS__' in window;

// Toast when a session flips to "waiting" — the app may be hidden in the tray.
export async function notifyWaiting(chatName: string): Promise<void> {
  if (!tauriAvailable) return;
  try {
    const { isPermissionGranted, requestPermission, sendNotification } = await import(
      '@tauri-apps/plugin-notification'
    );
    let granted = await isPermissionGranted();
    if (!granted) granted = (await requestPermission()) === 'granted';
    if (granted) {
      sendNotification({ title: 'Claude is waiting', body: `“${chatName}” needs your input` });
    }
  } catch {
    // notifications are best-effort
  }
}
