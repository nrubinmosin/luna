import type { CSSProperties } from 'react';
import { ACCENT, limitColor } from '../../shared/lib/format';
import { STATUS } from '../../shared/ui/status';
import { Icon } from '../../shared/ui/icons';
import { allChats, useChats } from '../chats/chats.store';
import { AccountsWindow, useAccountsWindow } from '../accounts/AccountsPanel';
import { useAccounts } from '../accounts/accounts.store';
import { currentLayout, currentSlots, usePanes } from '../panes/panes.store';
import { ui } from '../providers';
import { useUpdates } from '../updates/updates.store';
import { useCli } from '../updates/cli.store';
import { cliUpdateNow } from '../../ipc/commands';
import { PROVIDER_LABEL, PROVIDERS, accountKey, type Account } from '../../shared/types';
import { PowerChip } from '../power/PowerChip';
import { useShortcuts } from '../shortcuts/ShortcutsDialog';
import { useSettings } from '../settings/SettingsDialog';

const chipStyle = (loud: boolean) => ({
  flexGrow: 0,
  cursor: 'default',
  whiteSpace: 'nowrap' as const,
  fontVariantNumeric: 'tabular-nums',
  color: loud ? ACCENT : 'var(--dim)',
  fontWeight: loud ? 700 : 400
});

const pct = (got: number, total: number | null) =>
  total ? Math.min(100, Math.round((got / total) * 100)) : null;

/**
 * The bar's only word about updates, and only while there is news: a release
 * on offer, a download running, or a failure worth a retry. The quiet version
 * readouts that used to sit here full-time wrapped the bar onto a second
 * ragged line — they live in Settings now, where clicking through to them is
 * never urgent.
 */
function UpdateChips() {
  const { phase, next, got, total } = useUpdates();
  const clis = useCli(s => s.status);

  const chips = [];

  if (phase === 'available') {
    chips.push(
      <div
        key="app"
        onClick={() => useUpdates.getState().setAsking(true)}
        title={`Version ${next} is out — click for the release notes`}
        className="sb-field click"
        style={chipStyle(true)}
      >
        {`↑ ${next}`}
      </div>
    );
  } else if (phase === 'downloading') {
    const done = pct(got, total);
    chips.push(
      <div
        key="app"
        title={`Downloading ${next}… Luna closes on its own to install it.`}
        className="sb-field"
        style={chipStyle(true)}
      >
        {`↓ ${done == null ? '…' : `${done}%`}`}
      </div>
    );
  } else if (phase === 'error') {
    chips.push(
      <div
        key="app"
        onClick={() => void useUpdates.getState().check(true)}
        title="Update check failed — click to try again"
        className="sb-field click"
        style={chipStyle(false)}
      >
        ⚠ update
      </div>
    );
  }

  for (const p of PROVIDERS) {
    const cli = clis[p];
    const short = p === 'codex' ? 'codex' : 'claude';
    if (cli?.phase === 'downloading') {
      const done = pct(cli.got, cli.total);
      // The first download is the one that matters — without it a fresh install
      // has nothing to run; it is the only CLI state drawn loud.
      chips.push(
        <div
          key={`cli-${p}`}
          title={`Downloading ${PROVIDER_LABEL[p]} ${cli.latest ?? ''}… new chats use it once it lands.`}
          className="sb-field"
          style={chipStyle(!cli.version)}
        >
          {`${short} ↓ ${done == null ? '…' : `${done}%`}`}
        </div>
      );
    } else if (cli?.phase === 'error') {
      chips.push(
        <div
          key={`cli-${p}`}
          onClick={() => void cliUpdateNow(p)}
          title={`${PROVIDER_LABEL[p]} update failed — click to try again\n${cli.error ?? ''}`}
          className="sb-field click"
          style={chipStyle(false)}
        >
          {short} ⚠
        </div>
      );
    }
  }

  return <>{chips}</>;
}

/** One account as the status bar shows it: a dot, the name, two meters. */
function AccountGlance({ account }: { account: Account }) {
  const p = ui(account.provider);
  const windows: Array<{ k: string; v: number }> =
    account.limits.kind === 'claude'
      ? [{ k: '5h', v: account.limits.h5 }, { k: 'week', v: account.limits.week }]
      : account.limits.windows.slice(0, 2).map(w => ({ k: w.label === '5 hours' ? '5h' : w.label, v: w.used }));
  const dot = !account.signedIn
    ? 'oklch(.63 .19 25)'
    : p.worstLimit(account) >= 0.85
      ? 'oklch(.63 .19 25)'
      : account.haveUsage
        ? 'oklch(.64 .18 145)'
        : 'var(--faint)';

  return (
    <span
      className="sb-acc"
      title={
        `${account.name} — ${PROVIDER_LABEL[account.provider]}${account.plan ? `, ${account.plan}` : ''}\n` +
        (account.signedIn
          ? account.haveUsage
            ? windows.map(w => `${w.k}: ${Math.round(w.v * 100)}%`).join(' · ')
            : 'no usage data yet'
          : 'signed out')
      }
    >
      <span className="sb-dot" style={{ background: dot }} />
      <b>{account.name}</b>
      {account.provider === 'codex' && <span style={{ color: 'var(--faint)' }}>codex</span>}
      {!account.signedIn ? (
        <span style={{ color: 'oklch(.58 .2 25)' }}>signed out</span>
      ) : account.haveUsage ? (
        windows.map(w => (
          <span key={w.k} style={{ display: 'flex', alignItems: 'center', gap: 4 }}>
            <span className="sb-mini">
              <i style={{ width: `${Math.round(w.v * 100)}%`, ['--m' as string]: limitColor(w.v) } as CSSProperties} />
            </span>
            {Math.round(w.v * 100)}%
          </span>
        ))
      ) : (
        <span style={{ color: 'var(--faint)' }}>—</span>
      )}
    </span>
  );
}

/**
 * The window's status bar, the full width of it as XP drew one: what the
 * sessions are doing, every account at a glance, and the way into the rest.
 * The clock that used to sit here is gone — Windows keeps one in the taskbar.
 */
export function StatusBar() {
  const folders = useChats(s => s.folders);
  const accounts = useAccounts(s => s.accounts);

  const all = allChats(folders);
  const working = all.filter(c => c.status === 'working').length;
  const waiting = all.filter(c => c.status === 'waiting');
  const openAccounts = () => useAccountsWindow.getState().setOpen(true);

  // Each click goes to the next chat that is waiting, round the list.
  const jumpToWaiting = () => {
    if (!waiting.length) return;
    const active = useChats.getState().active;
    const at = waiting.findIndex(c => c.id === active);
    const chat = waiting[(at + 1) % waiting.length];
    let panes = usePanes.getState();
    if (chat.group !== panes.group) {
      panes.setGroup(chat.group);
      panes = usePanes.getState();
    }
    useChats.getState().setActive(chat.id);
    const seat = currentSlots(panes).indexOf(chat.id);
    if (seat >= 0 && seat < currentLayout(panes)) {
      panes.closePeek();
      panes.setActivePane(seat);
    } else {
      panes.showChat(chat.id);
    }
  };

  return (
    <div className="app-status">
      <div className={`sb-field${working ? ' loud' : ''}`} title={`${working} session${working === 1 ? '' : 's'} working`}>
        <span className="sb-dot" style={{ background: working ? STATUS.working.color : 'var(--faint)' }} />
        {working} working
      </div>
      <div
        onClick={jumpToWaiting}
        className={`sb-field${waiting.length ? ' waiting click' : ''}`}
        title={
          waiting.length
            ? `Waiting for you:\n${waiting.map(c => c.name).join('\n')}\nClick to go to ${waiting.length > 1 ? 'the next one' : 'it'}`
            : 'Nothing is waiting for you'
        }
      >
        <span className="sb-dot" style={{ background: waiting.length ? 'var(--wait)' : 'var(--faint)' }} />
        {waiting.length} waiting
      </div>

      <div onClick={openAccounts} className="sb-field grow click" title="Accounts — click to manage them">
        {accounts.length === 0 ? (
          <span style={{ color: 'var(--faint)' }}>No accounts yet — click to add one</span>
        ) : (
          accounts.map(a => <AccountGlance key={accountKey(a.provider, a.name)} account={a} />)
        )}
      </div>

      <UpdateChips />
      <div onClick={openAccounts} className="sb-field click" title="Accounts: sign in, limits, agents, add and delete">
        <Icon name="accounts" />
        Accounts
      </div>
      <PowerChip />
      <div
        onClick={() => useSettings.getState().setOpen(true)}
        title="Settings — accounts folder, board, new chats, versions and updates"
        className="sb-field click"
        style={{ padding: '0 6px' }}
      >
        <Icon name="gear" />
      </div>
      {/* Where a mouse user finds what the keyboard can do; F1 is the same. */}
      <div
        onClick={() => useShortcuts.getState().setOpen(true)}
        title="Shortcuts and mouse gestures — F1"
        className="sb-field click"
        style={{ padding: '0 6px' }}
      >
        <Icon name="help" />
      </div>
      <AccountsWindow />
    </div>
  );
}
