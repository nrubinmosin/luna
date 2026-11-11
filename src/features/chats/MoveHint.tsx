/**
 * The strip a pane shows when its chat's account runs low (a window at 90%
 * or more), or a usage limit stopped its last turn: a move to another
 * account is a click away. Shown only where another account of the CLI is
 * there to move to; dismissed for this run of the window.
 */
import type { Chat } from '../../shared/types';
import { findAccount, useAccounts } from '../accounts/accounts.store';
import { useMoveHints } from './moveHints.store';

/** The share of a window past which the strip shows. */
export const HINT_AT = 0.9;

export function MoveHint({ chat, onMove }: { chat: Chat; onMove: () => void }) {
  const limit = useMoveHints(s => s.limits[chat.id] ?? null);
  const dismissed = useMoveHints(s => !!s.dismissed[chat.id]);
  const accounts = useAccounts(s => s.accounts);
  if (dismissed || chat.movedTo) return null;
  if (!accounts.some(a => a.provider === chat.provider && a.name !== chat.account)) return null;
  const own = findAccount(accounts, chat.provider, chat.account);
  const high = own?.limits.filter(l => l.used >= HINT_AT).sort((a, b) => b.used - a.used)[0];
  if (!limit && !high) return null;
  const words = limit
    ? limit === 'usage_limit'
      ? 'This chat stopped on a usage limit.'
      : `This chat stopped on a usage limit (${limit.replace(/_/g, ' ')}).`
    : `${chat.account} is at ${Math.round((high?.used ?? 0) * 100)}% of its ${high?.label} limit.`;
  return (
    <div className="move-hint" role="note" data-testid="move-hint">
      <span>{words}</span>
      <span
        role="button"
        tabIndex={0}
        className="handoff-link hover-bg"
        onClick={onMove}
        onKeyDown={e => {
          if (e.key === 'Enter' || e.key === ' ') onMove();
        }}
      >
        Move to another account…
      </span>
      <button
        className="move-hint-close"
        aria-label="Dismiss"
        title="Not now"
        onClick={() => useMoveHints.getState().dismiss(chat.id)}
      >
        ×
      </button>
    </div>
  );
}
