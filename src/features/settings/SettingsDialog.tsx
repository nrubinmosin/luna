import { useEffect } from 'react';
import { createPortal } from 'react-dom';
import { useUpdates } from '../updates/updates.store';
import { useDialogHost } from '../../shared/ui/dialogHost';
import { BrowserSection } from '../browsers/BrowserSection';
import { useSettings } from './settings.store';
import { Section } from '../../shared/ui/Section';
import { AccountsFolderSection } from './sections/AccountsFolderSection';
import { NewChatsSection } from './sections/NewChatsSection';
import { HelpersSection } from './sections/HelpersSection';
import { TrustedRootsSection } from './sections/TrustedRootsSection';
import { BoardSection } from './sections/BoardSection';
import { NotificationsSection } from './sections/NotificationsSection';
import { WaitingSection } from './sections/WaitingSection';
import { DeletingSection } from './sections/DeletingSection';
import { VersionsSection } from './sections/VersionsSection';

/**
 * The app's settings, behind the status bar's gear. The version readouts and
 * their update clicks read better here, where there is room to say what state
 * each is in; the bar keeps only news (see StatusBar). Raised from the status
 * bar or the Accounts window.
 */
export function SettingsDialog() {
  const open = useSettings(s => s.open);
  return open ? <SettingsWindow onClose={() => useSettings.getState().setOpen(false)} /> : null;
}

function SettingsWindow({ onClose }: { onClose: () => void }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // The update-confirm dialog can stack on top; its own Escape handler
      // dismisses it, and this one must not take the settings down with it.
      if (e.key === 'Escape' && !useUpdates.getState().asking) onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);

  // Out of any draggable subtree, inside the [data-app] theme scope.
  const host = useDialogHost();
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
        placeItems: 'center',
        zIndex: 70
      }}
    >
      <div
        onClick={e => e.stopPropagation()}
        className="window"
        style={{ width: 430, boxShadow: 'var(--shadow), var(--border-window-outer), var(--border-window-inner)' }}
      >
        <div className="title-bar">
          <div className="title-bar-text">Settings</div>
          <div className="title-bar-controls">
            <button aria-label="Close" onClick={onClose} />
          </div>
        </div>
        <div className="window-body" style={{ maxHeight: 'calc(100vh - 120px)', overflowY: 'auto' }}>
          <AccountsFolderSection />
          <NewChatsSection />
          <HelpersSection />
          <Section label="Links">
            <BrowserSection />
          </Section>
          <TrustedRootsSection />
          <BoardSection />
          <NotificationsSection />
          <WaitingSection />
          <DeletingSection />
          <VersionsSection />
        </div>
      </div>
    </div>,
    host
  );
}
