import './shared/lib/storage-brand';
import 'xp.css/dist/XP.css';
import React from 'react';
import ReactDOM from 'react-dom/client';
import { App } from './app/App';
import { ErrorBoundary } from './app/ErrorBoundary';
import { installGlobalErrorLog, logInfo, watchMainThreadStalls } from './shared/lib/log';

installGlobalErrorLog();
watchMainThreadStalls();
logInfo('ui', 'renderer started');

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </React.StrictMode>
);
