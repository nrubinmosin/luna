import { useCallback, useEffect, useRef, useState } from 'react';
import { ChatList } from '../features/chats/ChatList';
import { OrphanSessions } from '../features/chats/OrphanSessions';
import { PaneGrid } from '../features/panes/PaneGrid';
import { StatusBar } from '../features/status-bar/StatusBar';
import { NewChatDialog } from '../features/new-chat/NewChatDialog';
import { useAgentBridge } from '../features/agents/bridge';
import { useAgentAccounts } from '../features/agents/agentAccounts.store';
import { AppFrame } from './AppFrame';
import { useAccounts } from '../features/accounts/accounts.store';
import { useNewChat } from '../features/new-chat/newchat.store';
import { useSessionWatch } from '../features/chats/useSessionWatch';
import { LoginModal } from '../features/accounts/LoginModal';
import { useUpdates } from '../features/updates/updates.store';
import { useCli } from '../features/updates/cli.store';
import { UpdateConfirm } from '../features/updates/UpdateConfirm';
import { CoreBanner } from '../features/core/CoreBanner';
import { useCore } from '../features/core/core.store';
import { useKeymap } from './keymap';
import { useAltPeek } from '../features/panes/altPeek';
import { ShortcutsDialog } from '../features/shortcuts/ShortcutsDialog';
import { SettingsDialog } from '../features/settings/SettingsDialog';
import { healScaleDrift } from './dpi';
import { Icon } from '../shared/ui/icons';
import { STRIP, useShell } from './shell.store';
import { uiHeartbeat } from '../ipc/commands';
import { onNotify, onOpenChat } from '../ipc/events';
import { peekFromNotice } from '../features/panes/navigate';
import { notify } from '../shared/lib/notify';
import { allChats, useChats } from '../features/chats/chats.store';
import { usePanes } from '../features/panes/panes.store';
import './theme.css';

/** How far in from the window's left edge the pointer brings a hidden
 *  sidebar out: the board's margin and the terminal's own padding, so it
 *  reaches in without covering a character of text. */
const EDGE = 24;

/**
 * The sidebar. Pinned, it holds its width in the row. Unpinned, it takes none:
 * resting the pointer at the window's left edge slides it out over the board
 * (which keeps its size), and leaving it slides it away again — never pinned
 * by the hover itself. The title bar's button pins and unpins.
 */
function Dock() {
  const pinned = useShell(s => s.pinned);
  const width = useShell(s => s.width);
  const openNewChat = useCallback(() => useNewChat.getState().openDialog(), []);
  const hover = useShell(s => s.overlay);
  const setHover = useShell.getState().setOverlay;
  const [resizing, setResizing] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  /** Which way the pending timer goes, so a pointer still moving does not
   *  keep putting the slide-out off. */
  const pending = useRef<'show' | 'hide' | null>(null);
  useEffect(() => () => clearTimeout(timer.current), []);

  const shown = pinned || hover;
  // Out after a beat, enough to let a pointer that only crosses the edge
  // pass; away the moment the pointer leaves it — there is nothing to wait
  // for once it has gone.
  const schedule = (to: 'show' | 'hide', ms: number) => {
    clearTimeout(timer.current);
    pending.current = to;
    timer.current = setTimeout(() => {
      pending.current = null;
      setHover(to === 'show');
    }, ms);
  };
  const enter = () => {
    if (!pinned && pending.current !== 'show') schedule('show', 40);
  };
  const leave = () => {
    if (pinned) return;
    clearTimeout(timer.current);
    pending.current = null;
    setHover(false);
  };
  const cancel = () => {
    if (pending.current !== 'show') return;
    clearTimeout(timer.current);
    pending.current = null;
  };

  // The edge band reaches over the panes' left padding too, where the band
  // element cannot go without covering the panes. Not over a title bar: its
  // first button sits right there and must stay clickable.
  useEffect(() => {
    if (pinned) return;
    const onMove = (e: MouseEvent) => {
      if (useShell.getState().overlay) return;
      const t = e.target instanceof Element ? e.target : null;
      if (e.clientX <= EDGE && !t?.closest('.title-bar, .dock')) enter();
      else if (e.clientX > EDGE && !t?.closest('.dock')) cancel();
    };
    // Capture: over a terminal whose CLI tracks the mouse, xterm takes the
    // move for itself and it never bubbles up here.
    window.addEventListener('mousemove', onMove, true);
    return () => window.removeEventListener('mousemove', onMove, true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pinned]);

  const startResize = (e: React.MouseEvent) => {
    e.preventDefault();
    const x0 = e.clientX, w0 = useShell.getState().width;
    const onMove = (ev: MouseEvent) => useShell.getState().setWidth(w0 + ev.clientX - x0);
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

  return (
    <div
      className="dock"
      data-mode={pinned ? 'pinned' : 'hidden'}
      data-shown={shown ? 'yes' : 'no'}
      onMouseEnter={enter}
      onMouseLeave={leave}
      style={{ width: pinned ? width : STRIP, transition: resizing ? 'none' : undefined }}
    >
      {!pinned && <div className="dock-strip" />}
      {/* Hidden rather than unmounted: the list keeps its scroll, and coming
          back costs a repaint. */}
      <div className="taskpane" style={{ width }} aria-hidden={!shown}>
        <button onClick={openNewChat} title="New chat" className="start-btn">
          <Icon name="plus" size={18} />
          New chat
        </button>
        <OrphanSessions />
        <ChatList />
      </div>
      {pinned && <div className="dock-grip" data-on={resizing ? 'yes' : 'no'} onMouseDown={startResize} title="Drag to resize" />}
    </div>
  );
}

export function App() {
  const pref = useShell(s => s.theme);
  const [sysDark, setSysDark] = useState(() => window.matchMedia('(prefers-color-scheme: dark)').matches);
  const modal = useNewChat(s => s.open);
  const loginFor = useAccounts(s => s.loginFor);

  useEffect(() => {
    const mql = window.matchMedia('(prefers-color-scheme: dark)');
    const onSys = (e: MediaQueryListEvent) => setSysDark(e.matches);
    mql.addEventListener('change', onSys);
    return () => mql.removeEventListener('change', onSys);
  }, []);

  useEffect(() => healScaleDrift(), []);

  useEffect(() => void useCore.getState().init(), []);

  useEffect(() => {
    const off = onNotify(n => void notify(n.title, n.body, n.chatId));
    const offOpen = onOpenChat(p => peekFromNotice(p.chatId));
    return () => {
      void off.then(f => f());
      void offOpen.then(f => f());
    };
  }, []);

  // A chat listed in a group the title bar does not offer could never be
  // reached; whatever put it there, the group is made rather than the chat
  // lost.
  useEffect(() => {
    const cover = () =>
      usePanes.getState().coverGroup(Math.max(0, ...allChats(useChats.getState().folders).map(c => c.group)));
    cover();
    return useChats.subscribe(cover);
  }, []);

  // Tells the Rust side the page is alive; a window that goes quiet while on
  // screen is rebuilt there (watchdog.rs).
  useEffect(() => {
    const beat = () => void uiHeartbeat().catch(() => {});
    beat();
    const t = setInterval(beat, 5000);
    return () => clearInterval(t);
  }, []);

  useEffect(() => {
    // The store owns the cadence: a fixed interval here would keep hitting a
    // rate-limited endpoint straight through the backoff it just scheduled.
    useAccounts.getState().startPolling();
    // Quietly: a release found at startup lights up a status-bar chip and
    // waits there.
    void useUpdates.getState().check(false);
    // The CLI readout feeds the settings dialog and the bar's download chip.
    useCli.getState().init();
    return () => useAccounts.getState().stopPolling();
  }, []);

  const openNewChat = useCallback(() => useNewChat.getState().openDialog(), []);
  const closeModal = useCallback(() => useNewChat.getState().close(), []);
  useKeymap({ openNewChat, closeModal });
  useAltPeek();
  useSessionWatch();
  useAgentBridge();
  useEffect(() => {
    void useAgentAccounts.getState().init();
  }, []);

  const theme = pref === 'system' ? (sysDark ? 'dark' : 'light') : pref;

  return (
    <div
      data-app
      data-theme={theme}
      style={{
        height: '100vh', background: 'var(--bg)', color: 'var(--fg)',
        fontFamily: 'var(--sans-serif)',
        fontSize: 'var(--ui)'
      }}
    >
      <AppFrame>
        <div style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column' }}>
          <div style={{ flex: 1, minHeight: 0, display: 'flex' }}>
            <Dock />
            <div style={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column' }}>
              <CoreBanner />
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
