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

// `pnpm dev` + `?demo` stands the UI up on invented chats and accounts, which
// is how the README screenshots are taken without anyone's real data in them.
// The whole branch goes away in a release build.
if (import.meta.env.DEV && new URLSearchParams(location.search).has('demo')) {
  void import('./dev/demo').then(m => {
    m.seedDemo(m.sceneFromUrl(location.search) ?? 'main');
    boot();
  });
} else {
  boot();
}
