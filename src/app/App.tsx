import { useCallback, useEffect, useRef, useState } from 'react';
import { ACCENT } from '../shared/lib/format';
import { ChatList } from '../features/chats/ChatList';
import { PaneGrid } from '../features/panes/PaneGrid';
import { StatusBar } from '../features/status-bar/StatusBar';
import { NewChatDialog } from '../features/new-chat/NewChatDialog';
import { usePanes, type Layout } from '../features/panes/panes.store';
import { useAccounts } from '../features/accounts/accounts.store';
import { useNewChat } from '../features/new-chat/newchat.store';
import { LoginModal } from '../features/accounts/LoginModal';
import { checkForUpdates } from '../shared/lib/updater';
import { useKeymap } from './keymap';
import './theme.css';

type ThemePref = 'system' | 'light' | 'dark';
const THEMES: Record<ThemePref, { glyph: string; title: string }> = {
  system: { glyph: '◐', title: 'Theme: system' },
  light: { glyph: '☀', title: 'Theme: light' },
  dark: { glyph: '☾', title: 'Theme: dark' }
};

function LayoutIcon({ n }: { n: Layout }) {
  const b = '1.4px solid var(--dim)';
  const box = { width: 14, height: 11, border: b, borderRadius: 2 } as const;
  if (n === 1) return <div style={box} />;
  if (n === 2)
    return (
      <div style={{ ...box, display: 'flex' }}>
        <div style={{ width: '50%', borderRight: b }} />
      </div>
    );
  if (n === 3)
    return (
      <div style={{ ...box, display: 'flex' }}>
        <div style={{ width: '52%', borderRight: b }} />
        <div style={{ flex: 1, display: 'flex', flexDirection: 'column' }}>
          <div style={{ height: '50%', borderBottom: b }} />
        </div>
      </div>
    );
  return (
    <div style={{ ...box, display: 'flex' }}>
      <div style={{ width: '50%', borderRight: b, display: 'flex', flexDirection: 'column' }}>
        <div style={{ height: '50%', borderBottom: b }} />
      </div>
      <div style={{ flex: 1, display: 'flex', flexDirection: 'column' }}>
        <div style={{ height: '50%', borderBottom: b }} />
      </div>
    </div>
  );
}

export function App() {
  const [pref, setPref] = useState<ThemePref>(() => (localStorage.getItem('llm-desktop.theme') as ThemePref) || 'system');
  const [sysDark, setSysDark] = useState(() => window.matchMedia('(prefers-color-scheme: dark)').matches);
  const [sidebar, setSidebar] = useState(280);
  const [resizing, setResizing] = useState(false);
  const modal = useNewChat(s => s.open);
  const layout = usePanes(s => s.layout);
  const setLayout = usePanes(s => s.setLayout);
  const loginFor = useAccounts(s => s.loginFor);
  const sidebarRef = useRef(sidebar);
  sidebarRef.current = sidebar;

  useEffect(() => {
    const mql = window.matchMedia('(prefers-color-scheme: dark)');
    const onSys = (e: MediaQueryListEvent) => setSysDark(e.matches);
    mql.addEventListener('change', onSys);
    return () => mql.removeEventListener('change', onSys);
  }, []);

  useEffect(() => {
    // The store owns the cadence: a fixed interval here would keep hitting a
    // rate-limited endpoint straight through the backoff it just scheduled.
    useAccounts.getState().startPolling();
    void checkForUpdates();
    return () => useAccounts.getState().stopPolling();
  }, []);

  useEffect(() => {
    localStorage.setItem('llm-desktop.theme', pref);
  }, [pref]);

  const openNewChat = useCallback(() => useNewChat.getState().openDialog(), []);
  const closeModal = useCallback(() => useNewChat.getState().close(), []);
  useKeymap({ openNewChat, closeModal });

  const startResize = (e: React.MouseEvent) => {
    e.preventDefault();
    const x0 = e.clientX, w0 = sidebarRef.current;
    const onMove = (ev: MouseEvent) => setSidebar(Math.min(480, Math.max(220, w0 + ev.clientX - x0)));
    const onUp = () => {
      window.removeEventListener('mousemove', onMove);
      window.removeEventListener('mouseup', onUp);
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      setResizing(false);
    };
    window.addEventListener('mousemove', onMove);
    window.addEventListener('mouseup', onUp);
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';
    setResizing(true);
  };

  const theme = pref === 'system' ? (sysDark ? 'dark' : 'light') : pref;
  const themeTitle = THEMES[pref].title + (pref === 'system' ? ` (${theme})` : '');

  return (
    <div
      data-app
      data-theme={theme}
      style={{
        ['--accent' as string]: ACCENT,
        height: '100vh', display: 'flex', background: 'var(--bg)', color: 'var(--fg)',
        fontFamily: 'var(--sans-serif)',
        fontSize: 13.5
      }}
    >
      <div style={{ width: sidebar, flex: 'none', background: 'var(--sidebar)', borderRight: '1px solid var(--window-frame)', display: 'flex', flexDirection: 'column', minHeight: 0 }}>
        <div style={{ flex: 'none', padding: '8px 8px 7px', display: 'flex', flexDirection: 'column', gap: 6 }}>
          <div style={{ display: 'flex', gap: 5 }}>
            <div
              onClick={openNewChat}
              className="hover-bright"
              style={{
                flex: 1, height: 27, borderRadius: 2, background: 'var(--accent)', color: '#fff',
                border: '1px solid var(--window-frame)',
                display: 'flex', alignItems: 'center', justifyContent: 'center', gap: 5, fontSize: 12.5,
                fontWeight: 700, cursor: 'default',
                boxShadow: 'inset -1px -1px rgba(0,0,0,.5), inset 1px 1px rgba(255,255,255,.55)'
              }}
            >
              <span style={{ fontSize: 14, lineHeight: 1, flex: 'none' }}>+</span>
              <span style={{ whiteSpace: 'nowrap' }}>New chat</span>
            </div>
            <div
              onClick={() => setPref(p => (p === 'system' ? 'light' : p === 'light' ? 'dark' : 'system'))}
              title={themeTitle}
              className="hover-bg xp-raised"
              style={{ width: 29, height: 27, flex: 'none', borderRadius: 2, background: 'var(--panel)', border: '1px solid var(--window-frame)', display: 'grid', placeItems: 'center', fontSize: 13, cursor: 'default' }}
            >
              {THEMES[pref].glyph}
            </div>
          </div>

          <div className="xp-sunken" style={{ display: 'flex', gap: 2, padding: 2, background: 'var(--chip)', borderRadius: 2 }}>
            {([1, 2, 3, 4] as Layout[]).map(n => (
              <div
                key={n}
                onClick={() => setLayout(n)}
                title={`${n} pane${n > 1 ? 's' : ''}`}
                className={layout !== n ? 'hover-bg' : undefined}
                style={{
                  flex: 1, height: 23, borderRadius: 2, display: 'grid', placeItems: 'center', cursor: 'default',
                  background: layout === n ? 'var(--bg)' : 'transparent',
                  boxShadow: layout === n ? 'var(--border-sunken-outer), var(--border-sunken-inner)' : 'none'
                }}
              >
                <LayoutIcon n={n} />
              </div>
            ))}
          </div>
        </div>

        <ChatList />
        <StatusBar />
      </div>

      <div
        onMouseDown={startResize}
        onDoubleClick={() => setSidebar(280)}
        title="Drag to resize · double-click to reset"
        style={{ width: 5, marginLeft: -3, marginRight: -2, flex: 'none', cursor: 'col-resize', zIndex: 20, background: resizing ? ACCENT : 'transparent' }}
      />

      <div style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column' }}>
        <PaneGrid />
      </div>

      {modal && <NewChatDialog />}
      {loginFor && <LoginModal account={loginFor} />}
    </div>
  );
}
