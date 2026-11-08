import { useEffect, useState } from 'react';
import { agoLabel } from '../../../shared/lib/format';
import { cliUpdateNow } from '../../../ipc/cli';
import { coreRestart, coreState, type CoreStateDto } from '../../../ipc/coreProcess';
import { onCoreState } from '../../../ipc/events';
import { useUpdates } from '../../updates/updates.store';
import { useCli } from '../../updates/cli.store';
import { PROVIDER_LABEL, type Provider } from '../../../shared/types';
import { Group, Row } from '../SettingsRows';

const pct = (got: number, total: number | null) => (total ? Math.min(100, Math.round((got / total) * 100)) : null);

/** One product's line in the updates section: name, state, one button. */
function VersionRow({
  name,
  version,
  status,
  statusTitle,
  error,
  action,
  actionLabel,
  primary,
  disabled
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
    <Row
      label={
        <>
          <b>{name}</b> <span style={{ color: 'var(--dim)', fontVariantNumeric: 'tabular-nums' }}>{version}</span>
        </>
      }
      desc={
        <div
          title={statusTitle ?? status}
          style={{
            whiteSpace: 'nowrap',
            overflow: 'hidden',
            textOverflow: 'ellipsis',
            color: error ? 'oklch(.58 .2 25)' : undefined
          }}
        >
          {status}
        </div>
      }
      control={
        <button
          onClick={action}
          disabled={disabled}
          className={`slim${primary ? ' primary' : ''}`}
          style={{ minWidth: 76 }}
        >
          {actionLabel}
        </button>
      }
    />
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
    // A push that lands before the first answer is the newer of the two.
    let heard = false;
    const off = onCoreState(st => {
      heard = true;
      setS(st);
    });
    void coreState().then(st => !heard && setS(st));
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

export function VersionsSection() {
  return (
    <Group
      about={{
        summary: 'How updates work',
        body: (
          <p>
            Luna checks for its own updates on launch; each CLI is re-checked every 6 hours. New chats spawn on a fresh
            CLI — running sessions keep the version they started with.
          </p>
        )
      }}
    >
      <LunaRow />
      <CoreRow />
      <CliRow provider="claude" />
      <CliRow provider="codex" />
    </Group>
  );
}
