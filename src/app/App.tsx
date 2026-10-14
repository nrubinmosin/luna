import { useCallback, useEffect, useRef, useState } from 'react';
import { ACCENT } from '../shared/lib/format';
import { ChatList } from '../features/chats/ChatList';
import { OrphanSessions } from '../features/chats/OrphanSessions';
import { PaneGrid } from '../features/panes/PaneGrid';
import { StatusBar } from '../features/status-bar/StatusBar';
import { NewChatDialog } from '../features/new-chat/NewChatDialog';
import { useAgentBridge } from '../features/agents/bridge';
import { useAgentAccounts } from '../features/agents/agentAccounts.store';
import {
  currentLayout,
  GROUP_LABELS,
  GROUPS,
  usePanes,
  type Layout
} from '../features/panes/panes.store';
import { AppFrame } from './AppFrame';
import { useAccounts } from '../features/accounts/accounts.store';
import { useNewChat } from '../features/new-chat/newchat.store';
import { useSessionWatch } from '../features/chats/useSessionWatch';
import { LoginModal } from '../features/accounts/LoginModal';
import { useUpdates } from '../features/updates/updates.store';
import { useCli } from '../features/updates/cli.store';
import { UpdateConfirm } from '../features/updates/UpdateConfirm';
import { useKeymap } from './keymap';
import { useAltPeek } from '../features/panes/altPeek';
import { ShortcutsDialog } from '../features/shortcuts/ShortcutsDialog';
import { SettingsDialog } from '../features/settings/SettingsDialog';
import { healScaleDrift } from './dpi';
import { Icon, ToolButton, type IconName } from '../shared/ui/icons';
import './theme.css';

type ThemePref = 'system' | 'light' | 'dark';
const THEMES: Record<ThemePref, { icon: IconName; title: string }> = {
  system: { icon: 'auto', title: 'Theme: system' },
  light: { icon: 'sun', title: 'Theme: light' },
  dark: { icon: 'moon', title: 'Theme: dark' }
};

/** A board's shape, drawn as a little XP window with its panes. */
function LayoutIcon({ n }: { n: Layout }) {
  const lines = { 1: '', 2: 'M9 5v9', 3: 'M9 5v9M9 9.5h7', 4: 'M9 5v9M2 9.5h14' }[n];
  return (
    <svg width="18" height="16" viewBox="0 0 18 16" aria-hidden style={{ display: 'block' }}>
      <rect x="1.5" y="1.5" width="15" height="13" rx="1.5" fill="#fff" stroke="#1d3f86" />
      <rect x="1.5" y="1.5" width="15" height="3.5" rx="1.5" fill="#2a6ff0" stroke="#1d3f86" />
      {lines && <path d={lines} stroke="#1d3f86" fill="none" />}
    </svg>
  );
}

export function App() {
  const [pref, setPref] = useState<ThemePref>(() => (localStorage.getItem('luna.theme') as ThemePref) || 'system');
  const [sysDark, setSysDark] = useState(() => window.matchMedia('(prefers-color-scheme: dark)').matches);
  const [sidebar, setSidebar] = useState(300);
  // The width is per-session, but a hidden sidebar is a deliberate choice about
  // what the window is for — it outlives the run, the way the theme does.
  const [hidden, setHidden] = useState(() => localStorage.getItem('luna.sidebar') === 'off');
  const [resizing, setResizing] = useState(false);
  const modal = useNewChat(s => s.open);
  const layout = usePanes(currentLayout);
  const setLayout = usePanes(s => s.setLayout);
  const group = usePanes(s => s.group);
  const setGroup = usePanes(s => s.setGroup);
  const resetGroup = usePanes(s => s.resetGroup);
  const loginFor = useAccounts(s => s.loginFor);
  const sidebarRef = useRef(sidebar);
  sidebarRef.current = sidebar;

  useEffect(() => {
    const mql = window.matchMedia('(prefers-color-scheme: dark)');
    const onSys = (e: MediaQueryListEvent) => setSysDark(e.matches);
    mql.addEventListener('change', onSys);
    return () => mql.removeEventListener('change', onSys);
  }, []);

  useEffect(() => healScaleDrift(), []);

  useEffect(() => {
    // The store owns the cadence: a fixed interval here would keep hitting a
    // rate-limited endpoint straight through the backoff it just scheduled.
    useAccounts.getState().startPolling();
    // Quietly: a release found at startup lights up a status-bar chip and
    // waits there. The modal this used to throw up arrived before the
    // panes had even painted, over whatever was mid-turn.
    void useUpdates.getState().check(false);
    // The CLI readout feeds the settings dialog and the bar's download chip.
    useCli.getState().init();
    return () => useAccounts.getState().stopPolling();
  }, []);

  useEffect(() => {
    localStorage.setItem('luna.theme', pref);
  }, [pref]);

  useEffect(() => {
    localStorage.setItem('luna.sidebar', hidden ? 'off' : 'on');
  }, [hidden]);

  const openNewChat = useCallback(() => useNewChat.getState().openDialog(), []);
  const closeModal = useCallback(() => useNewChat.getState().close(), []);
  useKeymap({ openNewChat, closeModal });
  useAltPeek();
  useSessionWatch();
  useAgentBridge();
  useEffect(() => {
    void useAgentAccounts.getState().init();
  }, []);

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
        height: '100vh', background: 'var(--bg)', color: 'var(--fg)',
        fontFamily: 'var(--sans-serif)',
        fontSize: 'var(--ui)'
      }}
    >
      <AppFrame>
        <div style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column' }}>
          <div style={{ flex: 1, minHeight: 0, display: 'flex' }}>
            {/* Hidden, the sidebar leaves this strip behind — the whole of it is the
                way back, so finding the sidebar again is never a hunt for a glyph. */}
            {hidden && (
              <div
                onClick={() => setHidden(false)}
                title="Show the sidebar"
                aria-label="Show the sidebar"
                role="button"
                className="taskpane"
                style={{
                  width: 26, flex: 'none', borderRight: '1px solid var(--window-frame)',
                  display: 'flex', justifyContent: 'center', paddingTop: 10, cursor: 'default'
                }}
              >
                <Icon name="show" size={18} />
              </div>
            )}

            <div
              className="taskpane"
              style={{
                width: sidebar, flex: 'none', borderRight: '1px solid var(--window-frame)', minHeight: 0,
                display: hidden ? 'none' : 'flex', flexDirection: 'column', gap: 10, padding: '10px 9px 10px'
              }}
            >
              <button
                onClick={openNewChat}
                title={
                  'New chat (Ctrl+N)\n' +
                  'Ctrl+Shift+N skips the dialog: same folder and account as the chat on screen, ' +
                  'settings as they stand'
                }
                className="start-btn"
                style={{ flex: 'none' }}
              >
                <Icon name="plus" size={18} />
                New chat
                <kbd>Ctrl+N</kbd>
              </button>

              <div className="tp-group">
                <div className="tp-head">
                  Board
                  <span style={{ fontWeight: 400, opacity: 0.7 }}>· group {GROUP_LABELS[group]}</span>
                </div>
                <div className="tp-body" style={{ display: 'flex', flexDirection: 'column', gap: 6, padding: 6 }}>
                  <div style={{ display: 'flex', gap: 6 }}>
                    <div className="seg" style={{ flex: 1 }}>
                      {([1, 2, 3, 4] as Layout[]).map(n => (
                        <button
                          key={n}
                          aria-pressed={layout === n}
                          onClick={() => setLayout(n)}
                          title={
                            `${n} pane${n > 1 ? 's' : ''} — Ctrl+Shift+${n}` +
                            (n === 1
                              ? '\nOne window: a click in the list swaps what it shows, Ctrl+<digit> too'
                              : `\nCtrl+1…${n} holds a pane up over the board for a moment`)
                          }
                        >
                          <LayoutIcon n={n} />
                        </button>
                      ))}
                    </div>
                    {/* One level above the layouts: each group remembers its own four
                        boards, so a set of chats can be parked and brought back whole. */}
                    <div className="seg roman" style={{ flex: 1 }}>
                      {GROUPS.map(g => (
                        <button
                          key={g}
                          aria-pressed={group === g}
                          onClick={() => setGroup(g)}
                          title={`Window group ${GROUP_LABELS[g]} — its own panes and splits\nCtrl+Tab goes to the next group`}
                        >
                          {GROUP_LABELS[g]}
                        </button>
                      ))}
                    </div>
                  </div>
                  <div style={{ display: 'flex', alignItems: 'center', gap: 2 }}>
                    <ToolButton
                      icon="even"
                      label="Even out"
                      title="Even out the splits of this board"
                      onClick={() => usePanes.getState().resetSplits()}
                    />
                    <ToolButton
                      icon="reset"
                      label="Clear"
                      title={`Clear group ${GROUP_LABELS[group]} — empties its panes, keeps every chat`}
                      onClick={() => resetGroup(group)}
                    />
                    <span style={{ flex: 1 }} />
                    <ToolButton
                      icon={THEMES[pref].icon}
                      title={themeTitle + '\nClick for the next one'}
                      onClick={() => setPref(p => (p === 'system' ? 'light' : p === 'light' ? 'dark' : 'system'))}
                    />
                    {/* Hides rather than unmounts: the list keeps its scroll and
                        coming back costs a repaint. */}
                    <ToolButton
                      icon="hide"
                      title="Hide the sidebar — the strip it leaves behind brings it back"
                      onClick={() => setHidden(true)}
                    />
                  </div>
                </div>
              </div>

              <OrphanSessions />
              <ChatList />
            </div>

            {!hidden && (
              <div
                onMouseDown={startResize}
                title="Drag to resize"
                style={{ width: 5, marginLeft: -3, marginRight: -2, flex: 'none', cursor: 'col-resize', zIndex: 20, background: resizing ? ACCENT : 'transparent' }}
              />
            )}

            <div style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column' }}>
              <PaneGrid />
            </div>
          </div>
          <StatusBar />
        </div>
      </AppFrame>

      {modal && <NewChatDialog />}
      {loginFor && <LoginModal account={loginFor} />}
      {/* Raised by the settings dialog or the status-bar chip alike. */}
      <UpdateConfirm />
      <ShortcutsDialog />
      <SettingsDialog />
    </div>
  );
}
