import { useLayoutEffect, useRef, useState, type CSSProperties } from 'react';
import { ACCENT, fmtReset, limitColor } from '../../shared/lib/format';
import { STATUS } from '../../shared/ui/status';
import { Icon } from '../../shared/ui/icons';
import { allChats, useChats } from '../chats/chats.store';
import { AccountsWindow, useAccountsWindow } from '../accounts/AccountsPanel';
import { useAccounts } from '../accounts/accounts.store';
import { jumpToWaiting } from '../panes/navigate';
import { useNow, worstLimit } from '../accounts/limits';
import { useUpdates } from '../updates/updates.store';
import { useCli } from '../updates/cli.store';
import { cliUpdateNow } from '../../ipc/commands';
import { PROVIDER_LABEL, PROVIDERS, accountKey, type Account } from '../../shared/types';
import { PowerChip } from '../power/PowerChip';
import { useShortcuts } from '../shortcuts/ShortcutsDialog';
import { useSettings } from '../settings/SettingsDialog';

const chipStyle = (loud: boolean) => ({
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
        <Icon name="update" />
        {next}
      </div>
    );
  } else if (phase === 'downloading' || phase === 'stopping') {
    const done = pct(got, total);
    chips.push(
      <div
        key="app"
        title={
          phase === 'stopping'
            ? `Stopping every session to install ${next}…`
            : `Downloading ${next}… Then the sessions are stopped and Luna closes to install it.`
        }
        className="sb-field"
        style={chipStyle(true)}
      >
        <Icon name="download" />
        {phase === 'stopping' ? 'stopping…' : done == null ? '…' : `${done}%`}
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
        <Icon name="warn" />
        update
      </div>
    );
  }

  for (const p of PROVIDERS) {
    const cli = clis[p];
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
          <Icon name={p} size={14} />
          <Icon name="download" />
          {done == null ? '…' : `${done}%`}
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
          <Icon name={p} size={14} />
          <Icon name="warn" />
        </div>
      );
    }
  }

  return <>{chips}</>;
}

/**
 * How much of each window the bar can afford, most first: everything; no
 * countdowns; no meters (key and number); only the fullest window.
 */
type Fit = 0 | 1 | 2 | 3;

/**
 * One account as the status bar shows it: a dot, the provider's icon, the
 * name, and every window it has — which one (5h, w, f), how full, and how long
 * until it resets — as far as `fit` leaves room for.
 */
function AccountGlance({ account, fit, now }: { account: Account; fit: Fit; now: number }) {
  const worst = worstLimit(account);
  const dot = account.sync === 'loading' && !account.haveUsage
    ? 'var(--faint)'
    : !account.signedIn || worst >= 0.85
    ? 'oklch(.63 .19 25)'
    : account.haveUsage
      ? 'oklch(.64 .18 145)'
      : 'var(--faint)';
  const windows = account.haveUsage ? account.limits : [];
  const shown = fit === 3 ? windows.filter(w => w.used === worst).slice(0, 1) : windows;

  return (
    <span
      className="sb-acc"
      title={
        `${account.name} — ${PROVIDER_LABEL[account.provider]}${account.plan ? `, ${account.plan}` : ''}\n` +
        (account.signedIn
          ? windows.length
            ? windows.map(w => `${w.label}: ${Math.round(w.used * 100)}%${w.resetAt ? `, resets in ${fmtReset(w.resetAt, now)}` : ''}`).join('\n')
            : 'no usage data yet'
          : 'signed out')
      }
    >
      <span className="sb-dot" style={{ background: dot }} />
      <Icon name={account.provider} size={14} />
      <b>{account.name}</b>
      {/* Signed-in is only known once the first answer is in: until then the
          account is loading, not signed out. */}
      {account.sync === 'loading' && !account.haveUsage ? (
        <span style={{ color: 'var(--faint)' }}>…</span>
      ) : !account.signedIn ? (
        <span style={{ color: 'oklch(.58 .2 25)' }}>signed out</span>
      ) : shown.length ? (
        shown.map(w => (
          <span key={w.id} className="sb-win">
            {/* Which window this is, before its meter: 5H, W(eek), F(able). */}
            <span className="sb-key">{w.short}</span>
            {fit < 2 && (
              <span className="sb-mini meter">
                <i style={{ width: `${Math.round(w.used * 100)}%`, ['--m' as string]: limitColor(w.used) } as CSSProperties} />
              </span>
            )}
            <span style={fit >= 2 ? { color: limitColor(w.used) } : undefined}>{Math.round(w.used * 100)}%</span>
            {fit === 0 && w.resetAt && <small>{fmtReset(w.resetAt, now)}</small>}
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
  const now = useNow();
  // Every window of every account, until they no longer fit; then the bar
  // gives up detail a step at a time (see Fit), never the windows themselves
  // until the last step. The width each step needed is kept, so widening the
  // window steps back up.
  const field = useRef<HTMLDivElement>(null);
  const [fit, setFit] = useState<Fit>(0);
  const needed = useRef<number[]>([]);
  useLayoutEffect(() => {
    const el = field.current;
    if (!el) return;
    const check = () => {
      needed.current[fit] = el.scrollWidth;
      if (el.scrollWidth > el.clientWidth + 1 && fit < 3) setFit((fit + 1) as Fit);
      else if (fit > 0 && el.clientWidth >= (needed.current[fit - 1] ?? Infinity)) setFit((fit - 1) as Fit);
    };
    check();
    const ro = new ResizeObserver(check);
    ro.observe(el);
    return () => ro.disconnect();
  });

  const all = allChats(folders);
  const working = all.filter(c => c.status === 'working').length;
  const waiting = all.filter(c => c.status === 'waiting');
  const openAccounts = () => useAccountsWindow.getState().setOpen(true);



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

      <div
        ref={field}
        onClick={openAccounts}
        className="sb-field grow click"
        title="Accounts — sign in, limits, agents, settings; click to open"
      >
        <Icon name="accounts" />
        {accounts.length === 0 ? (
          <span style={{ color: 'var(--faint)' }}>No accounts yet — click to add one</span>
        ) : (
          accounts.map(a => <AccountGlance key={accountKey(a.provider, a.name)} account={a} fit={fit} now={now} />)
        )}
      </div>

      <UpdateChips />
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
        title="Shortcuts and mouse gestures"
        className="sb-field click"
        style={{ padding: '0 6px' }}
      >
        <Icon name="help" />
      </div>
      <AccountsWindow />
    </div>
  );
}
