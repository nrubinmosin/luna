import { useEffect, useState } from 'react';
import { createPortal } from 'react-dom';
import type { ReactNode } from 'react';
import { agoLabel } from '../../shared/lib/format';
import { getAccountsRoot, pickFolder, setAccountsRoot, cliUpdateNow, type AccountsRootInfo } from '../../ipc/commands';
import { useAccounts } from '../accounts/accounts.store';
import { useUpdates } from '../updates/updates.store';
import { useCli } from '../updates/cli.store';

const pct = (got: number, total: number | null) =>
  total ? Math.min(100, Math.round((got / total) * 100)) : null;

function Section({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div style={{ marginBottom: 14 }}>
      <div
        style={{
          fontSize: 'var(--fs-1)', fontWeight: 700, textTransform: 'uppercase',
          letterSpacing: '.04em', color: 'var(--faint)', marginBottom: 6
        }}
      >
        {label}
      </div>
      {children}
    </div>
  );
}

/** One product's line in the updates section: name, state, one button. */
function VersionRow({
  name, version, status, statusTitle, error, action, actionLabel, primary, disabled
}: {
  name: string;
  version: string;
  status: string;
  statusTitle?: string;
  error?: boolean;
  action: () => void;
  actionLabel: string;
  primary?: boolean;
  disabled?: boolean;
}) {
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '4px 0' }}>
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ fontSize: 'var(--fs-4)', display: 'flex', alignItems: 'baseline', gap: 6 }}>
          <span style={{ fontWeight: 700 }}>{name}</span>
          <span style={{ color: 'var(--dim)', fontVariantNumeric: 'tabular-nums' }}>{version}</span>
        </div>
        <div
          title={statusTitle ?? status}
          style={{
            fontSize: 'var(--fs-2)', marginTop: 1, whiteSpace: 'nowrap', overflow: 'hidden',
            textOverflow: 'ellipsis', color: error ? 'oklch(.58 .2 25)' : 'var(--faint)'
          }}
        >
          {status}
        </div>
      </div>
      <button
        onClick={action}
        disabled={disabled}
        className={`slim${primary ? ' primary' : ''}`}
        style={{ flex: 'none', minWidth: 74 }}
      >
        {actionLabel}
      </button>
    </div>
  );
}

/** The Luna row wants the updater store; drawn from it wholesale like the old
 *  status-bar field was, since every phase shows up here now. */
function LunaRow() {
  const { phase, current, next, got, total, error, checkedAt } = useUpdates();

  const done = pct(got, total);
  const status =
    phase === 'checking'
      ? 'checking…'
      : phase === 'available'
        ? `version ${next} is out`
        : phase === 'downloading'
          ? `downloading ${next}… ${done == null ? '' : `${done}%`}`
          : phase === 'error'
            ? `update check failed — ${error ?? 'unknown error'}`
            : `checked ${agoLabel(checkedAt)}`;

  return (
    <VersionRow
      name="Luna"
      version={current || '—'}
      status={status}
      statusTitle={phase === 'error' ? (error ?? undefined) : undefined}
      error={phase === 'error'}
      primary={phase === 'available'}
      disabled={phase === 'checking' || phase === 'downloading'}
      actionLabel={phase === 'available' ? 'Update…' : 'Check'}
      action={() => {
        const s = useUpdates.getState();
        if (phase === 'available') s.setAsking(true);
        else void s.check(true);
      }}
    />
  );
}

function CliRow() {
  const s = useCli(st => st.status);

  if (!s) {
    return (
      <VersionRow
        name="Claude Code"
        version="—"
        status="no status yet"
        actionLabel="Check"
        action={() => void cliUpdateNow()}
      />
    );
  }

  const done = pct(s.got, s.total);
  const busy = s.phase === 'checking' || s.phase === 'downloading';
  const status =
    s.phase === 'downloading'
      ? `downloading ${s.latest ?? ''}… ${done == null ? '' : `${done}%`}`
      : s.phase === 'checking'
        ? 'checking…'
        : s.phase === 'error'
          ? `update failed — ${s.error ?? 'unknown error'}`
          : s.version
            ? `checked ${agoLabel(s.checkedAtMs)}`
            : 'no private copy yet — sessions use claude from PATH';

  return (
    <VersionRow
      name="Claude Code"
      version={s.version ?? '—'}
      status={status}
      statusTitle={s.phase === 'error' ? (s.error ?? undefined) : s.path}
      error={s.phase === 'error'}
      disabled={busy}
      actionLabel="Check"
      action={() => void cliUpdateNow()}
    />
  );
}

/** The accounts-folder mover, formerly its own fold-out card in the sidebar. */
function AccountsFolder() {
  const [info, setInfo] = useState<AccountsRootInfo | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void getAccountsRoot().then(setInfo);
  }, []);

  const apply = async (path: string) => {
    setError(null);
    try {
      setInfo(await setAccountsRoot(path));
      await useAccounts.getState().refresh();
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <>
      <div
        title={info?.path}
        style={{
          fontSize: 'var(--fs-3)', color: 'var(--dim)', whiteSpace: 'nowrap', overflow: 'hidden',
          textOverflow: 'ellipsis', direction: 'rtl', textAlign: 'left', marginBottom: 6
        }}
      >
        {/* rtl so the ellipsis eats the drive letter, not the folder name. */}
        <bdi>{info?.path ?? '…'}</bdi>
      </div>
      <div style={{ display: 'flex', gap: 6 }}>
        <button
          onClick={() => void pickFolder().then(p => { if (p) return apply(p); })}
          className="slim"
          style={{ minWidth: 74 }}
        >
          Change…
        </button>
        <button
          onClick={() => void apply('')}
          disabled={!info || info.isDefault}
          title="Back to Documents/claude-accounts"
          className="slim"
        >
          Default
        </button>
      </div>
      <div style={{ fontSize: 'var(--fs-1)', color: 'var(--faint)', marginTop: 5 }}>
        Only the location changes — move the account folders yourself. Running sessions keep their
        current paths.
      </div>
      {error && <div style={{ fontSize: 'var(--fs-2)', color: 'oklch(.58 .2 25)', marginTop: 4 }}>{error}</div>}
    </>
  );
}

/**
 * The app's settings, behind the sidebar's gear. Everything that used to
 * squat in the status bar as furniture — the two version readouts and their
 * update clicks — reads better here, where there is room to say what state
 * it is in; the bar keeps only news (see StatusBar).
 */
export function SettingsDialog({ onClose }: { onClose: () => void }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // The update-confirm dialog can stack on top; its own Escape handler
      // dismisses it, and this one must not take the settings down with it.
      if (e.key === 'Escape' && !useUpdates.getState().asking) onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);

  // Same host as ConfirmDialog, for the same reasons: out of any draggable
  // subtree, inside the [data-app] theme scope.
  const host = document.querySelector('[data-app]') ?? document.body;

  return createPortal(
    <div
      onClick={e => {
        e.stopPropagation();
        onClose();
      }}
      style={{
        position: 'fixed', inset: 0, background: 'rgba(0,0,0,.35)', backdropFilter: 'blur(1px)',
        display: 'grid', placeItems: 'center', zIndex: 70
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
        <div className="window-body">
          <Section label="Accounts folder">
            <AccountsFolder />
          </Section>
          <Section label="Versions & updates">
            <LunaRow />
            <CliRow />
            <div style={{ fontSize: 'var(--fs-1)', color: 'var(--faint)', marginTop: 4 }}>
              Luna checks for its own updates on launch; the Claude CLI re-checks every 6 hours.
              New chats spawn on a fresh CLI — running sessions keep the version they started with.
            </div>
          </Section>
        </div>
      </div>
    </div>,
    host
  );
}
