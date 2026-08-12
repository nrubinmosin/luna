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
      <div
        style={{
          height: '100vh', display: 'flex', flexDirection: 'column', gap: 14,
          alignItems: 'center', justifyContent: 'center', padding: 32,
          background: '#1a2226', color: '#d8e6e2',
          fontFamily: "-apple-system, 'Segoe UI', sans-serif", fontSize: 14.5
        }}
      >
        <div style={{ fontSize: 17, fontWeight: 640 }}>Something broke in the UI</div>
        <pre
          style={{
            maxWidth: 700, maxHeight: 220, overflow: 'auto', margin: 0, padding: 12,
            background: '#0f1518', borderRadius: 10, fontSize: 12.5, whiteSpace: 'pre-wrap'
          }}
        >
          {error.stack || String(error)}
        </pre>
        <div style={{ display: 'flex', gap: 10 }}>
          <div
            onClick={() => this.setState({ error: null })}
            style={{ padding: '8px 16px', borderRadius: 9, background: '#2c3a3f', cursor: 'default', fontSize: 13.5 }}
          >
            Try again
          </div>
          <div
            onClick={() => window.location.reload()}
            style={{ padding: '8px 16px', borderRadius: 9, background: '#3f7f6a', cursor: 'default', fontSize: 13.5, fontWeight: 590 }}
          >
            Reload
          </div>
        </div>
      </div>
    );
  }
}
