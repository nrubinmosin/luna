import { useState } from 'react';
import { agoLabel } from '../../../shared/lib/format';
import {
  cliCanaryDismiss,
  cliClearMark,
  cliMarkBad,
  cliMarkGood,
  cliSetChannel,
  cliUpdateNow,
  cliUseVersion,
  type CliChannel,
  type CliStatusDto,
  type CliVersionRowDto,
  type CliVersionsDto
} from '../../../ipc/cli';
import { CANARY_WATCH, canaryThrough, canaryWarning, useCli } from '../../updates/cli.store';
import { PROVIDER_LABEL, PROVIDERS, type Provider } from '../../../shared/types';
import { Group, List, ListItem, Row, type About } from '../SettingsRows';

const pct = (got: number, total: number | null) => (total ? Math.min(100, Math.round((got / total) * 100)) : null);

/** What the update pass is doing, or last did, in a line. */
function statusLine(s: CliStatusDto | null): string {
  if (!s) return 'no status yet';
  const done = pct(s.got, s.total);
  if (s.phase === 'downloading') return `downloading ${s.latest ?? ''}… ${done == null ? '' : `${done}%`}`;
  if (s.phase === 'checking') return 'checking…';
  if (s.phase === 'error') return `update failed — ${s.error ?? 'unknown error'}`;
  if (!s.version) return 'no working copy yet — sessions start once Luna has installed one';
  if (s.note) return s.note;
  const offered = s.latest && s.latest !== s.version && s.channel === 'knownGood' ? ` · ${s.latest} is out` : '';
  return `checked ${agoLabel(s.checkedAtMs)}${offered}`;
}

/** What one installed version is, in words after its number. */
function tags(r: CliVersionRowDto, v: CliVersionsDto | null): string[] {
  return [
    r.current && 'current',
    r.previous && 'previous',
    r.knownGood && 'known-good',
    r.good != null && 'marked good',
    canaryThrough(v, r.version) && `first ${CANARY_WATCH} sessions ok`,
    r.inUse && !r.current && 'in use',
    r.bad != null && 'bad'
  ].filter((t): t is string => !!t);
}

/** Why a version left installed from before the minimum rose has no Use. */
const BELOW_MIN = 'below the minimum safe version';

const tagClass = (t: string) => `set-tag${t === 'bad' ? ' set-tag-bad' : t === 'current' ? ' set-tag-on' : ''}`;

/** The marks on a version and how each is undone or made: a bad one is
 *  cleared, a good one unmarked; one with neither can be marked either way,
 *  good only if this Luna's list does not vouch for it already. */
function MarkButtons({
  row,
  busy,
  onGood,
  onBad,
  onClear
}: {
  row: CliVersionRowDto;
  busy: boolean;
  onGood: () => void;
  onBad: () => void;
  onClear: () => void;
}) {
  if (row.bad != null)
    return (
      <button className="slim" disabled={busy} onClick={onClear}>
        Clear bad
      </button>
    );
  return (
    <>
      {row.good != null ? (
        <button className="slim" disabled={busy} onClick={onClear}>
          Unmark good
        </button>
      ) : (
        !row.knownGood &&
        !row.belowMinSafe && (
          <button
            className="slim"
            disabled={busy}
            onClick={onGood}
            title="The known-good channel may install this version from now on"
          >
            Mark good
          </button>
        )
      )}
      <button className="slim" disabled={busy} onClick={onBad}>
        Mark bad
      </button>
    </>
  );
}

function VersionItem({
  row,
  versions,
  busy,
  onUse,
  marks
}: {
  row: CliVersionRowDto;
  versions: CliVersionsDto | null;
  busy: boolean;
  onUse: () => void;
  marks: { onGood: () => void; onBad: () => void; onClear: () => void };
}) {
  return (
    <ListItem>
      <span className="set-item-main">
        <span className="set-item-name">
          <span style={{ fontVariantNumeric: 'tabular-nums' }}>{row.version}</span>
          {tags(row, versions).map(t => (
            <span key={t} className={tagClass(t)}>
              {t}
            </span>
          ))}
        </span>
        {row.bad && (
          <span className="set-cmd" title={row.bad}>
            {row.bad}
          </span>
        )}
        {row.belowMinSafe && (
          <span className="set-cmd">
            {BELOW_MIN}
            {versions?.minSafe ? ` (${versions.minSafe})` : ''}
          </span>
        )}
      </span>
      <span className="set-item-actions">
        {!row.belowMinSafe && (
          <button className="slim" disabled={busy || row.current} onClick={onUse}>
            Use
          </button>
        )}
        <MarkButtons row={row} busy={busy} {...marks} />
      </span>
    </ListItem>
  );
}

/** A mark on a version no longer installed (the sweep took it, or the offline
 *  probe failed it), which only clearing is left for. */
function MarkItem({
  version,
  bad,
  why,
  busy,
  onClear
}: {
  version: string;
  bad: boolean;
  why: string;
  busy: boolean;
  onClear: () => void;
}) {
  return (
    <ListItem>
      <span className="set-item-main">
        <span className="set-item-name">
          <span style={{ fontVariantNumeric: 'tabular-nums' }}>{version}</span>
          <span className={tagClass(bad ? 'bad' : 'marked good')}>{bad ? 'bad' : 'marked good'}</span>
        </span>
        <span className="set-cmd" title={why}>
          {why}
        </span>
      </span>
      <span className="set-item-actions">
        <button className="slim" disabled={busy} onClick={onClear}>
          {bad ? 'Clear bad' : 'Unmark good'}
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
  // The one before, unless it is too old to run: then there is nothing to roll back to.
  const rollback = previous?.belowMinSafe ? undefined : previous;
  const pinned = v?.pinned ? v.versions.find(r => r.dir === v.pinned) : undefined;
  const warning = canaryWarning(v);
  const installed = new Set(v?.versions.map(r => r.version));
  const gone = [
    ...Object.entries(v?.bad ?? {}).map(([version, why]) => ({ version, why, bad: true })),
    ...Object.entries(v?.good ?? {}).map(([version, why]) => ({ version, why, bad: false }))
  ].filter(m => !installed.has(m.version));
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
          note={`${warning.failures.length} of the ${warning.watched} sessions started on it so far failed soon after starting.${previous && !rollback ? ` ${previous.version}, the one before, is ${BELOW_MIN}, so there is none to roll back to.` : ''} Running sessions keep their version either way.`}
          warn
          control={
            <>
              {rollback && (
                <button
                  className="slim primary"
                  disabled={busy}
                  onClick={() => act(() => cliUseVersion(provider, rollback.dir))}
                >
                  Roll back to {rollback.version}
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
            ? 'Versions this Luna release was checked with, and those you marked good.'
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
            {statusLine(s)}
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
                  versions={v}
                  busy={busy || passing}
                  onUse={() => act(() => cliUseVersion(provider, r.dir))}
                  marks={{
                    onGood: () => act(() => cliMarkGood(provider, r.version)),
                    onBad: () => act(() => cliMarkBad(provider, r.version)),
                    onClear: () => act(() => cliClearMark(provider, r.version))
                  }}
                />
              ))}
            />
            {gone.length > 0 && (
              <List
                label={`${name} marks on versions not installed`}
                empty=""
                items={gone.map(m => (
                  <MarkItem
                    key={m.version}
                    {...m}
                    busy={busy || passing}
                    onClear={() => act(() => cliClearMark(provider, m.version))}
                  />
                ))}
              />
            )}
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
        installs the newest version this Luna release lists as checked, or one you marked good here; a release with a
        security fix comes as a Luna update. On latest it installs each new release, after running it offline to see
        that it still knows every flag Luna passes. Nothing older than the oldest safe version a Luna release names ever
        runs.
      </p>
      <p>
        The first sessions on a new version are watched: if two of the first three fail soon after starting, Luna says
        so here and in the status bar, and once all three are past their start with none failed, the version says so. It
        never rolls back on its own. Use goes back to any installed version not below the oldest safe one (the one
        before is always kept) once its files match their checksums again, and holds there until you follow the channel
        again. Mark good lets the known-good channel install an installed version this Luna does not list, Mark bad
        keeps any version out, and Clear bad or Unmark good takes a mark back. Running sessions always keep the version
        they started with.
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
