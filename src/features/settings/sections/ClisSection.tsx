import { useState } from 'react';
import { agoLabel } from '../../../shared/lib/format';
import {
  cliCanaryDismiss,
  cliMarkBad,
  cliSetChannel,
  cliUpdateNow,
  cliUseVersion,
  type CliChannel,
  type CliStatusDto,
  type CliVersionRowDto
} from '../../../ipc/cli';
import { canaryWarning, useCli } from '../../updates/cli.store';
import { PROVIDER_LABEL, PROVIDERS, type Provider } from '../../../shared/types';
import { Group, List, ListItem, Row, type About } from '../SettingsRows';

const pct = (got: number, total: number | null) => (total ? Math.min(100, Math.round((got / total) * 100)) : null);

/** What the update pass is doing, or last did, in a line. */
function statusLine(s: CliStatusDto | null, provider: Provider): string {
  if (!s) return 'no status yet';
  const done = pct(s.got, s.total);
  if (s.phase === 'downloading') return `downloading ${s.latest ?? ''}… ${done == null ? '' : `${done}%`}`;
  if (s.phase === 'checking') return 'checking…';
  if (s.phase === 'error') return `update failed — ${s.error ?? 'unknown error'}`;
  if (!s.version) return `no private copy yet — sessions use ${provider} from PATH`;
  if (s.note) return s.note;
  const offered = s.latest && s.latest !== s.version && s.channel === 'knownGood' ? ` · ${s.latest} is out` : '';
  return `checked ${agoLabel(s.checkedAtMs)}${offered}`;
}

/** What one installed version is, in words after its number. */
function tags(r: CliVersionRowDto): string[] {
  return [
    r.current && 'current',
    r.previous && 'previous',
    r.knownGood && 'known-good',
    r.inUse && !r.current && 'in use',
    r.bad != null && 'bad'
  ].filter((t): t is string => !!t);
}

function VersionItem({
  row,
  busy,
  onUse,
  onBad
}: {
  row: CliVersionRowDto;
  busy: boolean;
  onUse: () => void;
  onBad: () => void;
}) {
  return (
    <ListItem>
      <span className="set-item-main">
        <span className="set-item-name">
          <span style={{ fontVariantNumeric: 'tabular-nums' }}>{row.version}</span>
          {tags(row).map(t => (
            <span key={t} className={`set-tag${t === 'bad' ? ' set-tag-bad' : t === 'current' ? ' set-tag-on' : ''}`}>
              {t}
            </span>
          ))}
        </span>
        {row.bad && (
          <span className="set-cmd" title={row.bad}>
            {row.bad}
          </span>
        )}
      </span>
      <span className="set-item-actions">
        <button className="slim" disabled={busy || row.current} onClick={onUse}>
          Use
        </button>
        <button className="slim" disabled={busy || row.bad != null} onClick={onBad}>
          Mark bad
        </button>
      </span>
    </ListItem>
  );
}

/** One CLI: its channel, its installed versions and what can be done to
 *  them, and the canary's warning while there is one. */
function CliGroup({ provider, about }: { provider: Provider; about?: About }) {
  const s = useCli(st => st.status[provider]);
  const v = useCli(st => st.versions[provider]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const name = PROVIDER_LABEL[provider];
  const channel: CliChannel = v?.channel ?? s?.channel ?? 'knownGood';
  const current = v?.versions.find(r => r.current);
  const previous = v?.versions.find(r => r.previous);
  const pinned = v?.pinned ? v.versions.find(r => r.dir === v.pinned) : undefined;
  const warning = canaryWarning(v);
  const passing = s?.phase === 'checking' || s?.phase === 'downloading';

  const act = (f: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    f()
      .catch(e => setError(String(e)))
      .finally(() => {
        setBusy(false);
        void useCli.getState().refresh(provider);
      });
  };

  return (
    <Group title={name} about={about}>
      {warning && (
        <Row
          label={`${name} ${warning.version} may be broken`}
          note={`${warning.failures.length} of the first ${warning.watched} sessions on it failed soon after starting. Running sessions keep their version either way.`}
          warn
          control={
            <>
              {previous && (
                <button
                  className="slim primary"
                  disabled={busy}
                  onClick={() => act(() => cliUseVersion(provider, previous.dir))}
                >
                  Roll back to {previous.version}
                </button>
              )}
              <button className="slim" disabled={busy} onClick={() => act(() => cliCanaryDismiss(provider))}>
                Keep it
              </button>
            </>
          }
        />
      )}
      <Row
        label="Channel"
        desc={
          channel === 'knownGood'
            ? 'Versions this Luna release was checked with.'
            : 'Each new release, once it passes an offline check.'
        }
        control={
          <select
            className="set-browser"
            aria-label={`${name} channel`}
            value={channel}
            disabled={busy}
            onChange={e => act(() => cliSetChannel(provider, e.target.value as CliChannel))}
          >
            <option value="knownGood">Known-good</option>
            <option value="latest">Latest</option>
          </select>
        }
      />
      <Row
        label={
          <>
            <b>Version</b>{' '}
            <span style={{ color: 'var(--dim)', fontVariantNumeric: 'tabular-nums' }}>
              {current?.version ?? s?.version ?? '—'}
            </span>
          </>
        }
        desc={
          <div
            title={s?.phase === 'error' ? (s.error ?? undefined) : s?.path}
            style={{
              whiteSpace: 'nowrap',
              overflow: 'hidden',
              textOverflow: 'ellipsis',
              color: s?.phase === 'error' ? 'var(--set-error)' : undefined
            }}
          >
            {statusLine(s, provider)}
          </div>
        }
        note={
          pinned ? `Held at ${pinned.version} by hand: updates wait until you follow the channel again.` : undefined
        }
        control={
          pinned ? (
            <button className="slim" disabled={busy} onClick={() => act(() => cliSetChannel(provider, channel))}>
              Follow channel
            </button>
          ) : (
            <button
              className="slim"
              disabled={busy || passing}
              onClick={() => void cliUpdateNow(provider)}
              style={{ minWidth: 76 }}
            >
              Check now
            </button>
          )
        }
        below={
          <>
            <List
              label={`${name} versions`}
              empty="No managed copy yet."
              items={(v?.versions ?? []).map(r => (
                <VersionItem
                  key={r.dir}
                  row={r}
                  busy={busy || passing}
                  onUse={() => act(() => cliUseVersion(provider, r.dir))}
                  onBad={() => act(() => cliMarkBad(provider, r.version))}
                />
              ))}
            />
            {error && <div className="set-error">{error}</div>}
          </>
        }
      />
    </Group>
  );
}

/** How the channels, the checks and going back work, under the last group. */
const ABOUT = {
  summary: 'How CLI versions work',
  body: (
    <>
      <p>
        Luna keeps its own copy of each CLI and checks for a new one every 6 hours. On the known-good channel it
        installs the newest version this Luna release lists as checked; a release with a security fix comes as a Luna
        update. On latest it installs each new release, after running it offline to see that it still knows every flag
        Luna passes. Nothing older than the oldest safe version a Luna release names ever runs.
      </p>
      <p>
        The first sessions on a new version are watched: if two of the first three fail soon after starting, Luna says
        so here and in the status bar. It never rolls back on its own. Use goes back to any installed version (the one
        before is always kept) once its files match their checksums again, and holds there until you follow the channel
        again. Running sessions always keep the version they started with.
      </p>
    </>
  )
};

export function ClisSection() {
  return (
    <>
      {PROVIDERS.map((p, i) => (
        <CliGroup key={p} provider={p} about={i === PROVIDERS.length - 1 ? ABOUT : undefined} />
      ))}
    </>
  );
}
