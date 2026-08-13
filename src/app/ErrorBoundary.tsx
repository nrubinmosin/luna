import { Component } from 'react';
import type { ErrorInfo, ReactNode } from 'react';
import { logError } from '../shared/lib/log';

// React tears down the whole root when an error escapes render *or* an effect
// cleanup — which is how closing a pane used to blank the window until F5.
// This keeps the failure local and visible instead of silently white-screening.
export class ErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    logError('react', `${error.stack ?? error} | ${info.componentStack ?? ''}`);
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;

    return (
      // Carries data-app of its own: the crash screen replaces the app's root,
      // so it would otherwise render outside every theme variable.
      <div
        data-app
        style={{
          height: '100vh', display: 'grid', placeItems: 'center', padding: 32,
          background: '#3a6ea5', fontFamily: 'var(--sans-serif)', fontSize: 'var(--ui)'
        }}
      >
        <div className="window" style={{ width: 640, maxWidth: '100%' }}>
          <div className="title-bar">
            <div className="title-bar-text">Luna — error</div>
            <div className="title-bar-controls">
              <button aria-label="Close" onClick={() => window.location.reload()} />
            </div>
          </div>
          <div className="window-body">
            <div style={{ display: 'flex', gap: 12, alignItems: 'flex-start' }}>
              <div style={{ fontSize: 'calc(var(--ui) * 2)', lineHeight: 1, color: '#c0392b' }}>⊗</div>
              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={{ fontWeight: 700, marginBottom: 6 }}>Something broke in the UI</div>
                <pre
                  style={{
                    maxHeight: 240, overflow: 'auto', margin: 0, padding: 8,
                    fontSize: 'var(--fs-3)', whiteSpace: 'pre-wrap'
                  }}
                >
                  {error.stack || String(error)}
                </pre>
              </div>
            </div>
            <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8, marginTop: 14 }}>
              <button onClick={() => this.setState({ error: null })}>Try again</button>
              <button onClick={() => window.location.reload()} style={{ fontWeight: 700 }}>
                Reload
              </button>
            </div>
          </div>
        </div>
      </div>
    );
  }
}
