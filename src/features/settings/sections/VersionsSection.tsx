import { useEffect, useState } from 'react';
import { agoLabel } from '../../../shared/lib/format';
import { coreRestart, coreState, type CoreStateDto } from '../../../ipc/coreProcess';
import { onCoreState } from '../../../ipc/events';
import { useUpdates } from '../../updates/updates.store';
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
            color: error ? 'var(--set-error)' : undefined
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

export function VersionsSection() {
  return (
    <Group
      about={{
        summary: 'How updates work',
        body: (
          <p>
            Luna checks for its own updates on launch. The CLIs have a page of their own, CLIs: which versions they
            follow, which are installed, and going back to one.
          </p>
        )
      }}
    >
      <LunaRow />
      <CoreRow />
    </Group>
  );
}
