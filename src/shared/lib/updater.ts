const tauriAvailable = '__TAURI_INTERNALS__' in window;

// Best-effort startup check against the configured GitHub Releases endpoint;
// silently no-ops until a release with latest.json actually exists.
export async function checkForUpdates(): Promise<void> {
  if (!tauriAvailable) return;
  try {
    const { check } = await import('@tauri-apps/plugin-updater');
    const update = await check();
    if (!update) return;
    if (window.confirm(`Update ${update.version} is available. Install and restart?`)) {
      await update.downloadAndInstall();
      const { relaunch } = await import('@tauri-apps/plugin-process');
      await relaunch();
    }
  } catch {
    // offline / no releases yet
  }
}
