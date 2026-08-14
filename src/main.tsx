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

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </React.StrictMode>
);
