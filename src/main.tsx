import 'xp.css/dist/XP.css';
import React from 'react';
import ReactDOM from 'react-dom/client';
import { App } from './app/App';
import { ErrorBoundary } from './app/ErrorBoundary';
import { installGlobalErrorLog, logInfo, watchMainThreadStalls } from './shared/lib/log';

installGlobalErrorLog();
watchMainThreadStalls();
logInfo('ui', 'renderer started');

// The webview's own context menu — Emoji, Writing direction, Send tab to your
// devices — is a browser's menu offered in an app that is not one, and none of
// it applies to a terminal. Off everywhere; the app's own right-click affordances
// would call preventDefault themselves anyway.
window.addEventListener('contextmenu', e => e.preventDefault());

const boot = () =>
  ReactDOM.createRoot(document.getElementById('root')!).render(
    <React.StrictMode>
      <ErrorBoundary>
        <App />
      </ErrorBoundary>
    </React.StrictMode>
  );

// xterm measures its cell on the face it finds when it opens, and a stand-in
// gives it the wrong cell for the real one; the refit on fonts.ready is a
// patch over that, not a cure. The face is local now (theme.css), so having it
// in hand before anything opens costs a few milliseconds. The timer covers a
// font store that never answers — a late font is a nuisance, a blank app is not.
const termFont = Promise.race([
  Promise.all([
    document.fonts.load("14px 'JetBrains Mono'"),
    document.fonts.load("bold 14px 'JetBrains Mono'")
  ]).catch(() => {}),
  new Promise(resolve => setTimeout(resolve, 1000))
]);

// `pnpm dev` + `?demo` stands the UI up on invented chats and accounts, which
// is how the README screenshots are taken without anyone's real data in them.
// The whole branch goes away in a release build.
if (import.meta.env.DEV && new URLSearchParams(location.search).has('demo')) {
  void Promise.all([import('./dev/demo'), termFont]).then(([m]) => {
    m.seedDemo(m.sceneFromUrl(location.search) ?? 'main');
    boot();
  });
} else {
  void termFont.then(boot);
}
