import { useEffect, useState } from 'react';
import { ACCENT } from '../../shared/lib/format';
import { cliStatus, cliUpdateNow, type CliStatusDto } from '../../ipc/commands';
import { onCliStatus } from '../../ipc/events';

const pct = (got: number, total: number | null) =>
  total ? Math.min(100, Math.round((got / total) * 100)) : null;

const ago = (at: number | null) => {
  if (!at) return 'never';
  const m = Math.round((Date.now() - at) / 60000);
  if (m < 1) return 'just now';
  if (m < 60) return `${m}m ago`;
  return `${Math.round(m / 60)}h ago`;
};

/**
 * Status-bar readout for Luna's own copy of the Claude CLI — the one every
 * session spawns. Sits next to the app's own version field and behaves the
 * same way: says which version runs, shows the download while one is in
 * flight, and a click asks for a check right now. The backend does the work
 * (cli.rs); this only draws what it reports.
 */
export function CliField() {
  const [s, setS] = useState<CliStatusDto | null>(null);

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let gone = false;
    void cliStatus().then(v => { if (!gone) setS(v); });
    void onCliStatus(setS).then(u => {
      if (gone) u();
      else unlisten = u;
    });
    return () => {
      gone = true;
      unlisten?.();
    };
  }, []);

  if (!s) return null;

  const done = pct(s.got, s.total);
  const label =
    s.phase === 'downloading'
      ? `cli ↓ ${done == null ? '…' : `${done}%`}`
      : s.phase === 'checking'
        ? 'cli …'
        : s.phase === 'error'
          ? `cli ⚠${s.version ? ` ${s.version}` : ''}`
          : s.version
            ? `cli ${s.version}`
            : 'cli —';

  const where = s.version ? `Luna's own copy:\n${s.path}` : 'No private copy yet — sessions use `claude` from PATH.';
  const title =
    s.phase === 'downloading'
      ? `Downloading Claude Code ${s.latest ?? ''}… running sessions keep their version; new chats use it once it lands.`
      : s.phase === 'error'
        ? `Claude CLI update failed — click to try again\n${s.error ?? ''}\n\n${where}`
        : `Claude Code ${s.version ?? '—'} · checked ${ago(s.checkedAtMs)}\n${where}\nClick to check for updates`;

  // The first download is the one that matters — without it a fresh install
  // has nothing to run; it is the only state drawn loud.
  const loud = s.phase === 'downloading' && !s.version;
  const busy = s.phase === 'checking' || s.phase === 'downloading';

  return (
    <div
      onClick={() => { if (!busy) void cliUpdateNow(); }}
      title={title}
      className="status-bar-field hover-bg"
      style={{
        flexGrow: 0, fontSize: 'var(--fs-2)', cursor: 'default', whiteSpace: 'nowrap',
        fontVariantNumeric: 'tabular-nums',
        color: loud ? ACCENT : s.phase === 'error' ? 'var(--dim)' : 'var(--faint)',
        fontWeight: loud ? 700 : 400
      }}
    >
      {label}
    </div>
  );
}
