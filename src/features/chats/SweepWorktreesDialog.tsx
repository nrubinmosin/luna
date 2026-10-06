import { useEffect, useState } from 'react';
import { agoLabel } from '../../shared/lib/format';
import { ConfirmDialog } from '../../shared/ui/ConfirmDialog';
import { inspectWorktrees, removeOrphanWorktrees, type SweepResultDto, type WorktreeInfoDto } from '../../ipc/commands';

/** What would be lost, in words; null when nothing would. */
function atStake(w: WorktreeInfoDto): string | null {
  if (w.broken) return 'not a git checkout any more — its contents could not be checked';
  const parts: string[] = [];
  if (w.uncommitted) parts.push(`${w.uncommitted} uncommitted file${w.uncommitted > 1 ? 's' : ''}`);
  if (w.uniqueCommits) parts.push(`${w.uniqueCommits} commit${w.uniqueCommits > 1 ? 's' : ''} on no other branch`);
  if (w.uniqueCommits == null) parts.push('commits could not be checked');
  return parts.length ? parts.join(', ') : null;
}

const leaf = (p: string) => p.split(/[\\/]/).filter(Boolean).pop() ?? p;

/**
 * The gate in front of the folder's "⌫ n" chip. A worktree nobody claims is
 * not necessarily junk — a chat deleted with "keep the worktree" leaves one on
 * purpose — so each is looked inside first, and only those with nothing to
 * lose start out ticked.
 */
export function SweepWorktreesDialog({
  folder, orphans, inUse, accountPaths, onClose
}: {
  folder: string;
  orphans: string[];
  inUse: string[];
  accountPaths: string[];
  onClose: () => void;
}) {
  const [infos, setInfos] = useState<WorktreeInfoDto[] | null>(null);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [phase, setPhase] = useState<'pick' | 'working' | 'done'>('pick');
  const [result, setResult] = useState<SweepResultDto | null>(null);

  useEffect(() => {
    let stale = false;
    void inspectWorktrees(folder, orphans)
      .catch(() => [] as WorktreeInfoDto[])
      .then(list => {
        if (stale) return;
        setInfos(list);
        setPicked(new Set(list.filter(w => !atStake(w)).map(w => w.path)));
      });
    return () => {
      stale = true;
    };
    // The list is the one the chip was showing when clicked; a rescan landing
    // while the dialog is open must not reshuffle it under the cursor.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const toggle = (path: string) =>
    setPicked(prev => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });

  const confirm = () => {
    if (phase === 'done') return onClose();
    if (phase === 'working' || !infos) return;
    if (picked.size === 0) return onClose();
    setPhase('working');
    void removeOrphanWorktrees(folder, inUse, accountPaths, [...picked])
      .catch(e => ({ removed: [], failed: [{ path: folder, error: String(e) }] }))
      .then(r => {
        setResult(r);
        // Nothing to read when everything went: close as a plain delete would.
        if (r.failed.length === 0) onClose();
        else setPhase('done');
      });
  };

  const risky = infos?.filter(w => picked.has(w.path) && atStake(w)).length ?? 0;

  const body =
    phase === 'done' && result ? (
      <>
        Deleted {result.removed.length}. These were not:
        <div style={{ marginTop: 8, display: 'flex', flexDirection: 'column', gap: 4 }}>
          {result.failed.map(f => (
            <div key={f.path} title={f.path} style={{ fontSize: 'var(--fs-3)' }}>
              <span style={{ fontWeight: 600, color: 'var(--fg)' }}>{leaf(f.path)}</span>{' '}
              <span style={{ color: 'oklch(.58 .2 25)' }}>{f.error}</span>
            </div>
          ))}
        </div>
      </>
    ) : (
      <>
        No chat or running session uses these. Deleting one removes its directory and its throwaway{' '}
        <code>worktree-…</code> / <code>codex-…</code> branch; work that exists nowhere else is gone for good.
        <div
          className="xp-field"
          style={{ marginTop: 8, background: '#fff', maxHeight: 240, overflowY: 'auto', padding: 2 }}
        >
          {!infos && <div style={{ padding: '6px 7px', fontSize: 'var(--fs-3)', color: '#666' }}>Looking inside…</div>}
          {infos?.length === 0 && (
            <div style={{ padding: '6px 7px', fontSize: 'var(--fs-3)', color: '#666' }}>Nothing left to sweep.</div>
          )}
          {infos?.map(w => {
            const stake = atStake(w);
            const id = `sweep-${w.path}`;
            return (
              <div key={w.path} title={w.path} style={{ display: 'flex', alignItems: 'flex-start', gap: 6, padding: '4px 5px' }}>
                {/* The box is drawn by xp.css on the label, so the input wants
                    an id and the label a matching `for`. */}
                <input type="checkbox" id={id} checked={picked.has(w.path)} onChange={() => toggle(w.path)} />
                <label htmlFor={id} style={{ display: 'block', cursor: 'default', minWidth: 0, color: '#000' }}>
                  <div style={{ fontSize: 'var(--fs-4)', fontWeight: 600, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                    {leaf(w.path)}
                    {w.branch && w.branch !== leaf(w.path) && (
                      <span style={{ fontWeight: 400, color: '#666' }}> · {w.branch}</span>
                    )}
                  </div>
                  <div style={{ fontSize: 'var(--fs-2)', marginTop: 1, color: stake ? 'oklch(.58 .2 25)' : '#666' }}>
                    {stake ?? 'clean — nothing that is not elsewhere'}
                    <span style={{ color: '#666' }}> · touched {agoLabel(w.touchedMs)}</span>
                  </div>
                </label>
              </div>
            );
          })}
        </div>
      </>
    );

  return (
    <ConfirmDialog
      title={phase === 'done' ? 'Stale worktrees' : 'Delete stale worktrees?'}
      body={body}
      extra={
        risky > 0 && phase === 'pick' ? (
          <div style={{ fontSize: 'var(--fs-3)', color: 'oklch(.58 .2 25)' }}>
            {risky} ticked worktree{risky > 1 ? 's' : ''} hold{risky === 1 ? 's' : ''} work that will be lost.
          </div>
        ) : undefined
      }
      confirmLabel={
        phase === 'done' ? 'Close' : phase === 'working' ? 'Deleting…' : picked.size ? `Delete ${picked.size}` : 'Close'
      }
      danger={phase === 'pick' && picked.size > 0}
      onConfirm={confirm}
      onCancel={() => phase !== 'working' && onClose()}
    />
  );
}
