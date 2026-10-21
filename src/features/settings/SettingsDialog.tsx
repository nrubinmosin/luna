import { useEffect, useState } from 'react';
import { createPortal } from 'react-dom';
import { create } from 'zustand';
import type { ReactNode } from 'react';
import { agoLabel } from '../../shared/lib/format';
import {
  getAccountsRoot, pickFolder, setAccountsRoot, cliUpdateNow, fetchBeforeWorktree, setFetchBeforeWorktree,
  coreRestart, coreState, type AccountsRootInfo, type CoreStateDto
} from '../../ipc/commands';
import { onCoreState } from '../../ipc/events';
import { useAccounts } from '../accounts/accounts.store';
import { useUpdates } from '../updates/updates.store';
import { useCli } from '../updates/cli.store';
import { PROVIDER_LABEL, type Provider } from '../../shared/types';
import { useNewChat } from '../new-chat/newchat.store';
import { useDeletePrefs } from '../chats/deletePrefs.store';
import { usePanes } from '../panes/panes.store';
import { useDialogHost } from '../../shared/ui/dialogHost';

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
          : phase === 'stopping'
            ? 'stopping the sessions to install…'
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
      disabled={phase === 'checking' || phase === 'downloading' || phase === 'stopping'}
      actionLabel={phase === 'available' ? 'Update…' : 'Check'}
      action={() => {
        const s = useUpdates.getState();
        if (phase === 'available') s.setAsking(true);
        else void s.check(true);
      }}
    />
  );
}

/** The core the window is attached to (corelink.rs): its build, how long it
 *  has run, and a way to start it afresh. */
function CoreRow() {
  const [s, setS] = useState<CoreStateDto | null>(null);
  useEffect(() => {
    void coreState().then(setS);
    const off = onCoreState(setS);
    return () => void off.then(f => f());
  }, []);
  const core = s?.core;
  const status =
    s?.state === 'up' && core
      ? `running, started ${agoLabel(core.started)} · pid ${core.pid}`
      : s?.state === 'starting'
        ? 'starting…'
        : `not running${s?.error ? ` — ${s.error}` : ''}`;
  return (
    <VersionRow
      name="Core"
      version={core ? `${core.version} · ${core.fingerprint.slice(0, 8)}` : '—'}
      status={status}
      error={s?.state === 'down'}
      disabled={s?.state === 'starting'}
      actionLabel={s?.state === 'up' ? 'Restart' : 'Start'}
      action={() => void coreRestart()}
    />
  );
}

function CliRow({ provider }: { provider: Provider }) {
  const s = useCli(st => st.status[provider]);
  const name = PROVIDER_LABEL[provider];

  if (!s) {
    return (
      <VersionRow
        name={name}
        version="—"
        status="no status yet"
        actionLabel="Check"
        action={() => void cliUpdateNow(provider)}
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
            : `no private copy yet — sessions use ${provider} from PATH`;

  return (
    <VersionRow
      name={name}
      version={s.version ?? '—'}
      status={status}
      statusTitle={s.phase === 'error' ? (s.error ?? undefined) : s.path}
      error={s.phase === 'error'}
      disabled={busy}
      actionLabel="Check"
      action={() => void cliUpdateNow(provider)}
    />
  );
}

/** One of the new-chat dialog's checkboxes, as it starts out. */
function StartRow({ label, which }: { label: string; which: 'worktree' | 'tools' }) {
  const on = useNewChat(s => s[which]);
  return (
    <div className="field-row" style={{ padding: '3px 0' }}>
      <input
        type="checkbox"
        id={`start-${which}`}
        checked={on}
        onChange={e => useNewChat.getState().setDefault(which, e.target.checked)}
      />
      <label htmlFor={`start-${which}`} style={{ cursor: 'default', fontSize: 'var(--fs-4)' }}>
        {label}
      </label>
    </div>
  );
}

/** Whether origin is fetched before Claude Code branches a worktree off it
 *  (Codex checks out the current commit instead). Kept by the Rust side,
 *  which does the fetch for chats agents start as well. */
function FetchRow() {
  const [on, setOn] = useState<boolean | null>(null);
  useEffect(() => {
    void fetchBeforeWorktree().then(setOn);
  }, []);
  return (
    <div className="field-row" style={{ padding: '3px 0' }}>
      <input
        type="checkbox"
        id="fetch-toggle"
        checked={on ?? true}
        disabled={on === null}
        onChange={e => void setFetchBeforeWorktree(e.target.checked).then(setOn)}
      />
      <label htmlFor="fetch-toggle" style={{ cursor: 'default', fontSize: 'var(--fs-4)' }}>
        Fetch origin before Claude Code makes a worktree
      </label>
    </div>
  );
}

/** Where the delete dialog's worktree checkbox starts. */
function DropWorktreeRow() {
  const on = useDeletePrefs(s => s.dropWorktree);
  return (
    <div className="field-row" style={{ padding: '3px 0' }}>
      <input
        type="checkbox"
        id="drop-worktree-toggle"
        checked={on}
        onChange={e => useDeletePrefs.getState().setDropWorktree(e.target.checked)}
      />
      <label htmlFor="drop-worktree-toggle" style={{ cursor: 'default', fontSize: 'var(--fs-4)' }}>
        Delete the worktree and its branch too
      </label>
    </div>
  );
}

/** Whether the board grows and shrinks with the chats on it. */
function AutoTileRow() {
  const on = usePanes(s => s.autoTile);
  return (
    <div className="field-row" style={{ padding: '3px 0' }}>
      <input
        type="checkbox"
        id="auto-tile-toggle"
        checked={on}
        onChange={e => usePanes.getState().setAutoTile(e.target.checked)}
      />
      <label htmlFor="auto-tile-toggle" style={{ cursor: 'default', fontSize: 'var(--fs-4)' }}>
        Auto-tile the board
      </label>
    </div>
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
          title="Back to Documents/luna-accounts"
          className="slim"
        >
          Default
        </button>
      </div>
      <div style={{ fontSize: 'var(--fs-1)', color: 'var(--faint)', marginTop: 5 }}>
        Claude Code accounts live under <code>anthropic/</code>, Codex accounts under <code>openai/</code>.
        Only the location changes — move the account folders yourself. Running sessions keep their
        current paths.
      </div>
      {error && <div style={{ fontSize: 'var(--fs-2)', color: 'oklch(.58 .2 25)', marginTop: 4 }}>{error}</div>}
    </>
  );
}

/**
 * The app's settings, behind the status bar's gear. The version readouts and
 * their update clicks read better here, where there is room to say what state
 * each is in; the bar keeps only news (see StatusBar). Raised from the status
 * bar or the Accounts window. `?demo=settings` opens it for its screenshot;
 * the check compiles away in a release build.
 */
export const useSettings = create<{ open: boolean; setOpen: (v: boolean) => void }>(set => ({
  open: import.meta.env.DEV && new URLSearchParams(location.search).get('demo') === 'settings',
  setOpen: v => set({ open: v })
}));

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
          <Section label="New chats">
            <StartRow label="Git worktree" which="worktree" />
            <StartRow label="Luna tools" which="tools" />
            <div style={{ fontSize: 'var(--fs-1)', color: 'var(--faint)', marginTop: 4 }}>
              How the two checkboxes start in a new chat, quick chats included. Flipping one is just for
              that chat; a folder that should start otherwise keeps its own — “keep for this folder” in
              the new-chat dialog.
            </div>
            <FetchRow />
            <div style={{ fontSize: 'var(--fs-1)', color: 'var(--faint)', marginTop: 4 }}>
              A worktree branches off origin’s default branch as last fetched, and Claude Code fetches it
              only once a day. With this on, Luna runs <code>git fetch</code> first — for Claude Code and
              Codex alike, ten seconds at most, never asking for a password. Only origin’s branches move;
              nothing you have checked out is touched.
            </div>
          </Section>
          <Section label="Board">
            <AutoTileRow />
            <div style={{ fontSize: 'var(--fs-1)', color: 'var(--faint)', marginTop: 4 }}>
              A new chat with no free pane grows the board by one pane, up to four, and lands in it;
              taking a chat off the board or deleting it shrinks the board by one. Off, a new chat with
              nowhere to go comes up on the sheet and the board stays as you arranged it.
            </div>
          </Section>
          <Section label="Deleting chats">
            <DropWorktreeRow />
            <div style={{ fontSize: 'var(--fs-1)', color: 'var(--faint)', marginTop: 4 }}>
              Where the worktree checkbox starts when you delete a chat that has one. It can still
              be flipped per chat; uncommitted work in a deleted worktree is lost.
            </div>
          </Section>
          <Section label="Versions & updates">
            <LunaRow />
            <CoreRow />
            <CliRow provider="claude" />
            <CliRow provider="codex" />
            <div style={{ fontSize: 'var(--fs-1)', color: 'var(--faint)', marginTop: 4 }}>
              Luna checks for its own updates on launch; each CLI is re-checked every 6 hours.
              New chats spawn on a fresh CLI — running sessions keep the version they started with.
            </div>
          </Section>
        </div>
      </div>
    </div>,
    host
  );
}
