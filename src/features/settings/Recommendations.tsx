import { useEffect, useState, type CSSProperties } from 'react';
import type { CheckDto } from '../../ipc/commands';
import { checkKey, useRecommendations } from './recommendations.store';

const small: CSSProperties = { fontSize: 'var(--fs-1)', color: 'var(--faint)' };
const link: CSSProperties = { ...small, textDecoration: 'underline', cursor: 'default' };
const WARN = 'oklch(.58 .2 25)';

const STATE: Record<CheckDto['status'], string | null> = {
  ok: null,
  off: null,
  blocked: 'Blocked',
  unknown: 'Could not read'
};

function Row({ check }: { check: CheckDto }) {
  const failed = useRecommendations(s => s.failed[checkKey(check)]);
  const [busy, setBusy] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const run = (confirmed: boolean) => {
    setBusy(true);
    setConfirming(false);
    void useRecommendations
      .getState()
      .apply(check, confirmed)
      .finally(() => setBusy(false));
  };
  const state = STATE[check.status];
  return (
    <div style={{ padding: '6px 0', borderBottom: '1px solid var(--line)', opacity: check.dismissed ? 0.6 : 1 }}>
      <div style={{ display: 'flex', gap: 6, alignItems: 'baseline', flexWrap: 'wrap', fontSize: 'var(--fs-4)' }}>
        <span style={{ fontWeight: 700 }}>{check.title}</span>
        {check.account && <code style={{ fontSize: 'var(--fs-1)' }}>{check.account}</code>}
        {state && <span style={{ fontSize: 'var(--fs-1)', fontWeight: 700, color: WARN }}>{state}</span>}
      </div>
      {check.why && <div style={{ fontSize: 'var(--fs-2)', color: 'var(--dim)', marginTop: 2 }}>{check.why}</div>}
      {check.current && (
        <div style={{ ...small, fontFamily: '"JetBrains Mono", monospace', marginTop: 2 }}>
          {check.current}
          {check.target && (
            <>
              {' '}
              → <span style={{ color: 'var(--fg)' }}>{check.target}</span>
            </>
          )}
        </div>
      )}
      {check.note && <div style={{ ...small, marginTop: 2 }}>{check.note}</div>}
      {check.refused && <div style={{ fontSize: 'var(--fs-1)', color: WARN, marginTop: 2 }}>{check.refused}</div>}
      {failed && <div style={{ fontSize: 'var(--fs-1)', color: WARN, marginTop: 2 }}>{failed}</div>}
      {confirming && (
        <div style={{ marginTop: 4, padding: 6, border: `1px solid ${WARN}` }}>
          <div style={{ fontSize: 'var(--fs-2)', marginBottom: 6 }}>{check.risk}</div>
          <div style={{ display: 'flex', gap: 6, justifyContent: 'flex-end' }}>
            <button onClick={() => setConfirming(false)}>Cancel</button>
            <button className="primary" onClick={() => run(true)}>
              Yes: {check.action}
            </button>
          </div>
        </div>
      )}
      {check.status !== 'ok' && !confirming && (
        <div style={{ display: 'flex', gap: 10, alignItems: 'center', marginTop: 4 }}>
          {check.action && !check.dismissed && (
            <button
              data-confirms={check.risk ? true : undefined}
              disabled={busy || !!check.refused}
              onClick={() => (check.risk ? setConfirming(true) : run(false))}
            >
              {busy ? 'Working…' : check.action}
            </button>
          )}
          <span
            className="hover-bg"
            style={link}
            onClick={() => void useRecommendations.getState().dismiss(check, !check.dismissed)}
          >
            {check.dismissed ? 'Show again' : 'Leave it'}
          </span>
        </div>
      )}
    </div>
  );
}

/** What Luna relies on, in the Recommendations window: what is off, what each is for, and one click. */
export function Recommendations() {
  const checks = useRecommendations(s => s.checks);
  useEffect(() => {
    void useRecommendations.getState().refresh();
  }, []);
  if (!checks) return <div style={small}>Reading…</div>;
  const shown = checks.filter(c => c.status !== 'ok' || c.note);
  const fine = checks.length - checks.filter(c => c.status !== 'ok').length;
  return (
    <div>
      {shown.map(c => (
        <Row key={checkKey(c)} check={c} />
      ))}
      <div style={{ ...small, marginTop: shown.length ? 6 : 0 }}>
        {shown.length === 0 ? `All ${fine} checks pass: ` : `${fine} more pass. `}
        What Luna’s features rely on, in its own settings, the accounts’ files and any policy on this computer. Nothing
        changes until you press its button, and a session already running may go on as it was until it restarts.
      </div>
    </div>
  );
}
