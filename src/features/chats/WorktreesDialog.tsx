import { useEffect, useState } from 'react';
import { createPortal } from 'react-dom';
import type { CSSProperties } from 'react';
import { agoLabel, tail2 } from '../../shared/lib/format';
import { Icon } from '../../shared/ui/icons';
import { inspectWorktrees, removeOrphanWorktrees, type WorktreeInfoDto } from '../../ipc/commands';
import { isClaudeWorktree, scanScope, useWorktrees } from './worktrees.store';

type Info = WorktreeInfoDto & { folder: string };

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
/** Codex's checkouts are all named after the repo; the bucket tells them apart. */
const codexWorktree = /[\\/]worktrees[\\/]([0-9a-f]{4})[\\/]([^\\/]+)[\\/]?$/i;
const nameOf = (p: string) => {
  const m = codexWorktree.exec(p);
  return m ? `${m[1]}\\${m[2]}` : leaf(p);
};

const PROVIDERS = [
  { key: 'claude', label: 'Claude Code', match: isClaudeWorktree },
  { key: 'codex', label: 'Codex', match: (p: string) => !isClaudeWorktree(p) }
] as const;

const faint: CSSProperties = { fontSize: 'var(--fs-2)', color: 'var(--faint)' };
const red = 'oklch(.58 .2 25)';

/**
 * The title bar's Worktrees window: every worktree Claude Code or Codex left
 * behind with no chat or running session in it, by folder and by CLI. A
 * worktree nobody claims is not necessarily junk — a chat deleted with "keep
 * the worktree" leaves one on purpose — so each is looked inside first, and
 * only those with nothing to lose start out ticked.
 */
export function WorktreesDialog() {
  const open = useWorktrees(s => s.open);
  if (!open) return null;
  return <Window />;
}

function Window() {
  const close = () => useWorktrees.getState().setOpen(false);
  const [infos, setInfos] = useState<Info[] | null>(null);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState<Set<string>>(new Set());
  const [errors, setErrors] = useState<Map<string, string>>(new Map());
  // A row's own Delete on a worktree that holds work asks once more, in place.
  const [armed, setArmed] = useState<string | null>(null);

  useEffect(() => {
    let stale = false;
    void (async () => {
      // Fresh rather than the button's count, which may be a minute old.
      await useWorktrees.getState().rescan();
      const byFolder = new Map<string, string[]>();
      for (const w of useWorktrees.getState().stale) byFolder.set(w.folder, [...(byFolder.get(w.folder) ?? []), w.path]);
      const lists = await Promise.all(
        [...byFolder].map(([folder, paths]) =>
          inspectWorktrees(folder, paths)
            .catch(() => [] as WorktreeInfoDto[])
            .then(list => list.map(w => ({ ...w, folder })))
        )
      );
      if (stale) return;
      const all = lists.flat();
      setInfos(all);
      setPicked(new Set(all.filter(w => !atStake(w)).map(w => w.path)));
    })();
    return () => {
      stale = true;
    };
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') close();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  const working = busy.size > 0;

  const remove = async (paths: string[]) => {
    if (!infos || paths.length === 0) return;
    setArmed(null);
    setBusy(new Set(paths));
    const byFolder = new Map<string, string[]>();
    for (const w of infos.filter(w => paths.includes(w.path))) byFolder.set(w.folder, [...(byFolder.get(w.folder) ?? []), w.path]);
    // Re-checked in the core against what is in use now: the list on screen
    // may be minutes old by the time the button is pressed.
    const { inUse, accountPaths } = scanScope();
    const results = await Promise.all(
      [...byFolder].map(([folder, ps]) =>
        removeOrphanWorktrees(folder, inUse, accountPaths, ps).catch(e => ({
          removed: [],
          failed: ps.map(path => ({ path, error: String(e) }))
        }))
      )
    );
    const removed = new Set(results.flatMap(r => r.removed));
    setInfos(prev => prev?.filter(w => !removed.has(w.path)) ?? prev);
    setPicked(prev => new Set([...prev].filter(p => !removed.has(p))));
    setErrors(prev => {
      const next = new Map(prev);
      for (const p of removed) next.delete(p);
      for (const f of results.flatMap(r => r.failed)) next.set(f.path, f.error);
      return next;
    });
    setBusy(new Set());
    void useWorktrees.getState().rescan();
  };

  const toggle = (paths: string[], on: boolean) =>
    setPicked(prev => {
      const next = new Set(prev);
      for (const p of paths) {
        if (on) next.add(p);
        else next.delete(p);
      }
      return next;
    });

  const selectBy = (which: (w: Info) => boolean) => setPicked(new Set((infos ?? []).filter(which).map(w => w.path)));
  const risky = infos?.filter(w => picked.has(w.path) && atStake(w)).length ?? 0;
  const folders = [...new Set((infos ?? []).map(w => w.folder))].sort();
  const host = document.querySelector('[data-app]') ?? document.body;

  return createPortal(
    <div
      onClick={e => {
        e.stopPropagation();
        if (!working) close();
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
        style={{ width: 620, maxWidth: 'calc(100vw - 32px)', boxShadow: 'var(--shadow), var(--border-window-outer), var(--border-window-inner)' }}
      >
        <div className="title-bar">
          <div className="title-bar-text">Stale worktrees</div>
          <div className="title-bar-controls">
            <button aria-label="Close" onClick={close} />
          </div>
        </div>
        <div className="window-body">
          <div style={{ fontSize: 'var(--fs-4)', lineHeight: 1.5, color: 'var(--dim)' }}>
            Worktrees Claude Code and Codex made for your folders that no chat or running session uses. Deleting one
            removes its directory, and for Claude Code its throwaway <code>worktree-…</code> branch; work that exists
            nowhere else is gone for good.
          </div>

          {infos && infos.length > 0 && (
            <div style={{ marginTop: 10, display: 'flex', alignItems: 'center', gap: 6, fontSize: 'var(--fs-3)' }}>
              <span style={{ color: 'var(--dim)' }}>Select:</span>
              <button onClick={() => selectBy(() => true)}>All</button>
              <button onClick={() => selectBy(w => !atStake(w))}>Clean only</button>
              <button onClick={() => selectBy(() => false)}>None</button>
            </div>
          )}

          <div className="xp-field" style={{ marginTop: 8, maxHeight: 'calc(100vh - 320px)', minHeight: 60, overflowY: 'auto', padding: 2 }}>
            {!infos && <div style={{ padding: '6px 7px', ...faint, fontSize: 'var(--fs-3)' }}>Looking inside…</div>}
            {infos?.length === 0 && (
              <div style={{ padding: '6px 7px', ...faint, fontSize: 'var(--fs-3)' }}>Nothing to clean: every worktree is in use.</div>
            )}
            {folders.map(folder => (
              <div key={folder} style={{ marginBottom: 6 }}>
                <div title={folder} style={{ display: 'flex', alignItems: 'center', gap: 5, padding: '5px 5px 3px', fontWeight: 700, fontSize: 'var(--fs-4)', color: 'var(--fg)' }}>
                  <Icon name="folder" size={14} />
                  {tail2(folder).leaf}
                  <span style={{ ...faint, fontWeight: 400, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{folder}</span>
                </div>
                {PROVIDERS.map(p => {
                  const rows = infos!.filter(w => w.folder === folder && p.match(w.path));
                  if (rows.length === 0) return null;
                  const all = rows.every(w => picked.has(w.path));
                  const groupId = `wt-${folder}-${p.key}`;
                  return (
                    <div key={p.key} style={{ marginLeft: 12 }}>
                      <div style={{ display: 'flex', alignItems: 'center', gap: 6, padding: '3px 5px' }}>
                        <input type="checkbox" id={groupId} checked={all} disabled={working} onChange={() => toggle(rows.map(w => w.path), !all)} />
                        <label htmlFor={groupId} style={{ display: 'flex', alignItems: 'center', gap: 5, cursor: 'default', fontWeight: 600, fontSize: 'var(--fs-3)', color: 'var(--dim)' }}>
                          <Icon name={p.key} size={12} />
                          {p.label} · {rows.length}
                        </label>
                      </div>
                      {rows.map(w => (
                        <Row
                          key={w.path}
                          w={w}
                          picked={picked.has(w.path)}
                          busy={busy.has(w.path)}
                          disabled={working}
                          armed={armed === w.path}
                          error={errors.get(w.path)}
                          onToggle={() => toggle([w.path], !picked.has(w.path))}
                          onDelete={() => (atStake(w) && armed !== w.path ? setArmed(w.path) : void remove([w.path]))}
                        />
                      ))}
                    </div>
                  );
                })}
              </div>
            ))}
          </div>

          {risky > 0 && (
            <div style={{ marginTop: 8, fontSize: 'var(--fs-3)', color: red }}>
              {risky} ticked worktree{risky > 1 ? 's' : ''} hold{risky === 1 ? 's' : ''} work that will be lost.
            </div>
          )}
          <div style={{ marginTop: 12, display: 'flex', justifyContent: 'flex-end', gap: 8 }}>
            <button onClick={close} disabled={working}>Close</button>
            <button className="danger" disabled={working || picked.size === 0} onClick={() => void remove([...picked])}>
              {working ? 'Deleting…' : `Delete selected (${picked.size})`}
            </button>
          </div>
        </div>
      </div>
    </div>,
    host
  );
}

function Row({
  w, picked, busy, disabled, armed, error, onToggle, onDelete
}: {
  w: Info;
  picked: boolean;
  busy: boolean;
  disabled: boolean;
  armed: boolean;
  error?: string;
  onToggle: () => void;
  onDelete: () => void;
}) {
  const stake = atStake(w);
  const id = `wt-${w.path}`;
  return (
    <div title={w.path} style={{ display: 'flex', alignItems: 'flex-start', gap: 6, padding: '3px 5px 3px 18px' }}>
      {/* The box is drawn by xp.css on the label, so the input wants an id
          and the label a matching `for`. */}
      <input type="checkbox" id={id} checked={picked} disabled={disabled} onChange={onToggle} />
      <label htmlFor={id} style={{ display: 'block', flex: 1, cursor: 'default', minWidth: 0, color: 'var(--fg)' }}>
        <div style={{ fontSize: 'var(--fs-4)', fontWeight: 600, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
          {nameOf(w.path)}
          {w.branch && w.branch !== leaf(w.path) && <span style={{ fontWeight: 400, color: 'var(--faint)' }}> · {w.branch}</span>}
        </div>
        <div style={{ fontSize: 'var(--fs-2)', marginTop: 1, color: stake ? red : 'var(--faint)' }}>
          {stake ?? 'clean — nothing that is not elsewhere'}
          <span style={{ color: 'var(--faint)' }}> · touched {agoLabel(w.touchedMs)}</span>
        </div>
        {error && <div style={{ fontSize: 'var(--fs-2)', marginTop: 1, color: red }}>Not deleted: {error}</div>}
      </label>
      <button
        className={armed ? 'danger' : undefined}
        disabled={disabled}
        onClick={onDelete}
        title={stake && !armed ? `Holds work: ${stake}. Asks once more.` : 'Delete this worktree'}
        style={{ flex: 'none', minWidth: 0, padding: '0 10px' }}
      >
        {busy ? 'Deleting…' : armed ? 'Lose it?' : 'Delete'}
      </button>
    </div>
  );
}
