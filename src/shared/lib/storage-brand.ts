// Persisted state used to sit under the pre-rebrand `llm-desktop.` prefix.
// Imported first in main.tsx: the zustand stores read localStorage the moment
// their modules load, so the move has to happen before any of them are pulled in.
const RENAMED: [string, string][] = [
  ['llm-desktop.theme', 'luna.theme'],
  ['llm-desktop.chats', 'luna.chats'],
  ['llm-desktop.panes', 'luna.panes']
];

try {
  for (const [before, after] of RENAMED) {
    const value = localStorage.getItem(before);
    if (value === null) continue;
    // A key already written under the new name is the newer truth; never clobber it.
    if (localStorage.getItem(after) === null) localStorage.setItem(after, value);
    localStorage.removeItem(before);
  }
} catch {
  // Storage being unavailable costs the user their layout, not the app.
}
