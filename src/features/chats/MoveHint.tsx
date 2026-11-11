/**
 * The strip a pane shows when its chat's account runs low (a window at 90%
 * or more), or a usage limit stopped its last turn: a move to another
 * account is a click away. Shown only where another account of the CLI is
 * there to move to; dismissed for this run of the window.
 */
import { useEffect, useState } from 'react';
import { moveStep } from '../../ipc/moving';
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
  // Over the terminal's top rows, not above them: a strip that took room
  // would shrink the pane's terminal, and a new chat there could stand at a
  // prompt too cramped to be read as one (agents/handoff/moving.rs).
  return (
    <div style={{ position: 'relative', height: 0, zIndex: 3 }} data-testid="move-hint-slot">
      <div
        className="move-hint"
        role="note"
        data-testid="move-hint"
        style={{ position: 'absolute', left: 0, right: 0, top: 0 }}
      >
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
    </div>
  );
}

/** How long a moved chat's opening waits before the person is offered to
 *  have Luna's line typed now (the core logs the screen at the same mark). */
export const STALL_MS = 2 * 60_000;

/**
 * On a chat a move opened, while Luna's line waits for a prompt its screen
 * check does not see: what it waits on, and "Type it now" for the person who
 * sees the prompt. Over the terminal, as the strip above.
 */
export function OpeningHint({ chat, now: given }: { chat: Chat; now?: number }) {
  const m = useMoveHints(s => s.opening[chat.id]);
  const [asked, setAsked] = useState(false);
  // The clock, read off the render: a tick every few seconds is enough.
  const [ticked, setTicked] = useState(0);
  useEffect(() => {
    if (given !== undefined) return;
    const t = setInterval(() => setTicked(Date.now()), 5000);
    return () => clearInterval(t);
  }, [given]);
  const now = given ?? ticked;
  if (!m || m.phase !== 'rowCreated' || !m.waiting || now - (m.waitingSince ?? now) < STALL_MS) return null;
  return (
    <div style={{ position: 'relative', height: 0, zIndex: 3 }}>
      <div
        className="move-hint"
        role="note"
        data-testid="opening-hint"
        style={{ position: 'absolute', left: 0, right: 0, top: 0 }}
      >
        <span title={m.waiting}>Luna has not typed its line yet: {m.waiting}.</span>
        <button
          disabled={asked}
          onClick={() => {
            setAsked(true);
            void moveStep(m.chatId, m.moveId, 'typeNow').catch(() => setAsked(false));
          }}
        >
          Type it now
        </button>
      </div>
    </div>
  );
}
