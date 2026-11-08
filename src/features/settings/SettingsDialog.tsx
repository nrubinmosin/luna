import { useEffect, useRef, type KeyboardEvent as ReactKeyboardEvent, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { useUpdates } from '../updates/updates.store';
import { useDialogHost } from '../../shared/ui/dialogHost';
import { Icon, type IconName } from '../../shared/ui/icons';
import { BrowsersSection, OpenLinksSection } from '../browsers/BrowserSection';
import { SETTINGS_PAGES, useSettings, type SettingsPage } from './settings.store';
import { useModal } from '../../shared/ui/modal';
import { AccountsFolderSection } from './sections/AccountsFolderSection';
import { NewChatsSection } from './sections/NewChatsSection';
import { WorktreesSection } from './sections/WorktreesSection';
import { HelpersSection } from './sections/HelpersSection';
import { TrustedRootsSection } from './sections/TrustedRootsSection';
import { BoardSection } from './sections/BoardSection';
import { NotificationsSection } from './sections/NotificationsSection';
import { WaitingSection } from './sections/WaitingSection';
import { VersionsSection } from './sections/VersionsSection';
import './settings.css';

/** Each page: its name in the list, its icon, and its groups. */
const PAGES: Record<SettingsPage, { title: string; icon: IconName; body: () => ReactNode }> = {
  general: {
    title: 'General',
    icon: 'gear',
    body: () => (
      <>
        <BoardSection />
        <NotificationsSection />
        <WaitingSection />
        <AccountsFolderSection />
      </>
    )
  },
  chats: {
    title: 'Chats',
    icon: 'plus',
    body: () => (
      <>
        <NewChatsSection />
        <WorktreesSection />
      </>
    )
  },
  agents: {
    title: 'Agents & trust',
    icon: 'key',
    body: () => (
      <>
        <HelpersSection />
        <TrustedRootsSection />
      </>
    )
  },
  links: {
    title: 'Links',
    icon: 'show',
    body: () => (
      <>
        <OpenLinksSection />
        <BrowsersSection />
      </>
    )
  },
  updates: { title: 'Updates', icon: 'update', body: () => <VersionsSection /> }
};

/**
 * The app's settings, behind the status bar's gear: a list of pages on the
 * left, the page on the right, and only the page scrolls. The version
 * readouts and their update clicks read better here, where there is room to
 * say what state each is in; the bar keeps only news (see StatusBar). Raised
 * from the status bar or the Accounts window.
 */
export function SettingsDialog() {
  const open = useSettings(s => s.open);
  return open ? <SettingsWindow onClose={() => useSettings.getState().setOpen(false)} /> : null;
}

function SettingsWindow({ onClose }: { onClose: () => void }) {
  const page = useSettings(s => s.page);
  const current = useUpdates(s => s.current);
  const nav = useRef<HTMLDivElement>(null);
  const dialog = useRef<HTMLDivElement>(null);
  // Out of any draggable subtree, inside the [data-app] theme scope.
  const host = useDialogHost();

  // Modal: the rest of the app is inert while Settings is up, Tab goes round
  // inside it, and the focus goes back to what opened it on close. Keyboard
  // users land on the page list, ready for the arrows. Popups opened later
  // (the exception menu) land beside it in the host and stay live; the update
  // confirm is a modal of its own and suspends Settings while it is up.
  const trap = useModal(dialog, host, '[role="tab"][aria-selected="true"]');

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // The update-confirm dialog can stack on top; its own Escape handler
      // dismisses it, and this one must not take the settings down with it.
      if (e.key === 'Escape' && !useUpdates.getState().asking) onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);

  // The page list is a vertical tab list: the arrows, Home and End move
  // between pages and show each one; Tab goes on into the page.
  const onNavKey = (e: ReactKeyboardEvent) => {
    const at = SETTINGS_PAGES.indexOf(page);
    const to =
      e.key === 'ArrowDown'
        ? (at + 1) % SETTINGS_PAGES.length
        : e.key === 'ArrowUp'
          ? (at - 1 + SETTINGS_PAGES.length) % SETTINGS_PAGES.length
          : e.key === 'Home'
            ? 0
            : e.key === 'End'
              ? SETTINGS_PAGES.length - 1
              : null;
    const next = to === null ? undefined : SETTINGS_PAGES[to];
    if (to === null || !next) return;
    e.preventDefault();
    useSettings.getState().setPage(next);
    nav.current?.querySelectorAll<HTMLElement>('[role="tab"]')[to]?.focus();
  };

  if (!host) return null;

  return createPortal(
    <div
      onClick={e => {
        e.stopPropagation();
        onClose();
      }}
      className="scrim"
      style={{
        position: 'fixed',
        inset: 0,
        background: 'rgba(0,0,0,.35)',
        backdropFilter: 'blur(1px)',
        display: 'grid',
        // Top-anchored where a full-height dialog would start, so a shorter
        // page does not make the dialog jump.
        alignItems: 'start',
        justifyItems: 'center',
        paddingTop: 'max(24px, calc((100vh - 720px) / 2))',
        zIndex: 70
      }}
    >
      <div
        ref={dialog}
        onClick={e => e.stopPropagation()}
        onKeyDown={trap}
        className="window settings"
        role="dialog"
        aria-modal="true"
        aria-labelledby="settings-title"
      >
        <div className="title-bar">
          <div className="title-bar-text" id="settings-title">
            Settings
          </div>
          <div className="title-bar-controls">
            <button aria-label="Close" onClick={onClose} />
          </div>
        </div>
        <div className="settings-body">
          <div className="settings-nav">
            <div
              ref={nav}
              className="settings-tabs"
              role="tablist"
              aria-orientation="vertical"
              aria-label="Settings pages"
              onKeyDown={onNavKey}
            >
              {SETTINGS_PAGES.map(id => (
                <button
                  key={id}
                  role="tab"
                  id={`settings-tab-${id}`}
                  aria-selected={id === page}
                  aria-controls={`settings-panel-${id}`}
                  tabIndex={id === page ? 0 : -1}
                  onClick={() => useSettings.getState().setPage(id)}
                >
                  <Icon name={PAGES[id].icon} size={16} />
                  {PAGES[id].title}
                </button>
              ))}
            </div>
            <div className="settings-nav-foot">Luna {current || '—'}</div>
          </div>
          {/* Every page stays mounted, the others hidden: a draft typed on one
              (the waiting marker, a browser of your own) is still there after
              a look at another. */}
          {SETTINGS_PAGES.map(id => (
            <div
              key={id}
              className="settings-page"
              id={`settings-panel-${id}`}
              role="tabpanel"
              aria-labelledby={`settings-tab-${id}`}
              hidden={id !== page}
            >
              <h2 className="settings-page-title">{PAGES[id].title}</h2>
              <div className="settings-scroll">{PAGES[id].body()}</div>
            </div>
          ))}
        </div>
      </div>
    </div>,
    host
  );
}
