import { useEffect, useState } from 'react';
import { backgroundMove, backgroundMoveTargets, type BackgroundHelperDto, type MoveTargetsDto } from '../../ipc/agents';
import { ConfirmDialog } from '../../shared/ui/ConfirmDialog';
import { logWarn } from '../../shared/lib/log';
import { helperName } from './helperSummary';
import { suggestedChats, type NameOf } from './ownership';

/**
 * The person's chats a helper may move to, with why not where it may not
 * (agents/reparent.rs `move_targets`): one picked and Move clicked moves it,
 * with the helpers under it; nothing moves before. A refusal stays in the
 * dialog, nothing changed.
 */
export function MoveHelperDialog({
  helper,
  nameOf,
  onClose
}: {
  helper: BackgroundHelperDto;
  nameOf: NameOf;
  onClose: () => void;
}) {
  const [targets, setTargets] = useState<MoveTargetsDto | null>(null);
  const [chosen, setChosen] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [refused, setRefused] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    backgroundMoveTargets(helper.id)
      .then(t => {
        if (live) setTargets(t);
      })
      .catch(e => {
        logWarn('agents', `listing where a helper may move failed: ${String(e)}`);
        if (live) setRefused(String(e));
      });
    return () => {
      live = false;
    };
  }, [helper.id]);
  // Its own chat aside: the core lists it, as not one to move to.
  const list = (targets?.targets ?? []).filter(t => t.id !== helper.parent);
  // A chat that asked for it is picked to start with; Move is still the person's.
  const ok = new Set(list.filter(t => t.ok).map(t => t.id));
  const picked = chosen ?? suggestedChats(helper).find(c => ok.has(c)) ?? null;

  const move = () => {
    if (!picked || busy) return;
    setBusy(true);
    setRefused(null);
    backgroundMove(helper.id, picked)
      .then(onClose)
      .catch(e => {
        logWarn('agents', `moving a helper failed: ${String(e)}`);
        setRefused(String(e));
        setBusy(false);
      });
  };

  const name = helperName(helper);
  return (
    <ConfirmDialog
      title={`Move "${name.length > 38 ? `${name.slice(0, 38)}…` : name}" to another chat?`}
      body={
        <>
          It goes with every helper under it, and the chat you pick manages it from then on: its conversation, worktree
          and settings stay, a turn under way goes on. Its chat now is told once and can no longer send to it, answer,
          stop or delete it.
        </>
      }
      extra={
        <div className="move-helper" style={{ display: 'grid', gap: 6, fontSize: 'var(--fs-4)' }}>
          {!targets && !refused && <div style={{ color: 'var(--dim)' }}>Looking at your chats…</div>}
          {targets?.blocked && (
            <div style={{ color: 'var(--wait)' }} data-testid="move-blocked">
              It cannot move now: {targets.blocked}
            </div>
          )}
          {targets && !targets.blocked && list.length === 0 && (
            <div style={{ color: 'var(--dim)' }}>You have no other chat to move it to.</div>
          )}
          {!targets?.blocked &&
            list.map(t => (
              <div key={t.id} style={{ display: 'grid', gap: 1 }}>
                <div style={{ display: 'flex', alignItems: 'center', gap: 6, minWidth: 0 }}>
                  <input
                    type="radio"
                    name={`move-${helper.id}`}
                    id={`move-${helper.id}-${t.id}`}
                    disabled={!t.ok || busy}
                    checked={picked === t.id}
                    onChange={() => setChosen(t.id)}
                  />
                  <label
                    htmlFor={`move-${helper.id}-${t.id}`}
                    title={nameOf(t.id)}
                    style={{ minWidth: 0, color: t.ok ? undefined : 'var(--faint)' }}
                  >
                    {/* Cut inside the label: cut on the label, it would take the radio xp.css draws there too. */}
                    <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                      {nameOf(t.id)}
                    </span>
                  </label>
                </div>
                {t.why && (
                  <div style={{ paddingLeft: 22, fontSize: 'var(--fs-1)', color: 'var(--faint)' }}>{t.why}</div>
                )}
              </div>
            ))}
          {refused && (
            <div style={{ color: 'var(--wait)' }} role="alert">
              Not moved: {refused}
            </div>
          )}
        </div>
      }
      confirmLabel={busy ? 'Moving…' : 'Move'}
      danger={false}
      confirmDisabled={!picked || busy}
      onConfirm={move}
      onCancel={onClose}
    />
  );
}
