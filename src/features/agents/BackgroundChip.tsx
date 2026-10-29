import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { useShallow } from 'zustand/react/shallow';
import { backgroundDelete, backgroundKill, backgroundPromote, type BackgroundHelperDto } from '../../ipc/commands';
import { Icon } from '../../shared/ui/icons';
import { useDialogHost } from '../../shared/ui/dialogHost';
import { sinceLabel } from '../../shared/lib/format';
import { logWarn } from '../../shared/lib/log';
import { useBackground } from './background.store';

/** A state as a person reads it. */
const STATE_LABEL: Record<string, string> = {
  working: 'working',
  waiting: 'asks for permission',
  question: 'asks a question',
  idle: 'done',
  background: 'a job still runs',
  exited: 'gone'
};

/** How long a chip glows after a helper of its chat was spawned. */
const NEW_MS = 5000;

/** What it runs on: "sonnet · medium · team · worktree". */
function setupLine(h: BackgroundHelperDto): string {
  return [h.model ?? 'default model', h.effort, h.account, h.worktree ? 'worktree' : null].filter(Boolean).join(' · ');
}

/** "working 4m" on its turn, "done · 12m" after it: since the turn or the spawn. */
function stateLine(h: BackgroundHelperDto, now: number): string {
  const label = STATE_LABEL[h.state] ?? h.state;
  if (h.turnStartedMs) return `${label} ${sinceLabel(h.turnStartedMs, now)}`;
  return `${label} · up ${sinceLabel(h.startedMs, now)}`;
}

/** The time now, again every `ms` while mounted. */
function useNow(ms: number): number {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    const t = window.setInterval(() => setNow(Date.now()), ms);
    return () => window.clearInterval(t);
  }, [ms]);
  return now;
}

/** What a helper asks, on one line. */
function askLine(h: BackgroundHelperDto): string | null {
  const a = h.ask;
  if (!a) return null;
  if (a.kind === 'permission') return [a.tool, a.detail].filter(Boolean).join(': ') || null;
  return a.questions?.map(q => q.text).join(' · ') ?? null;
}

/**
 * The background helpers of one chat (background.rs): a count on its row,
 * and on a click a list of them with what each is doing, to open one as a
 * chat of its own, stop it or delete it. Its agent is the one told what they
 * say; this is for a person who wants to look.
 */
export function BackgroundChip({ parentId }: { parentId: string }) {
  const helpers = useBackground(useShallow(s => s.helpers.filter(h => h.parent === parentId)));
  const [anchor, setAnchor] = useState<HTMLElement | null>(null);
  const newest = Math.max(0, ...helpers.map(h => h.startedMs));
  const [fresh, setFresh] = useState(false);
  // A spawn is the one moment nothing else on screen marks: glow for a while.
  useEffect(() => {
    const left = newest + NEW_MS - Date.now();
    // Also when the newest helper went inside its glow: the cleanup took the
    // timer that would have put the glow out.
    if (left <= 0) {
      setFresh(false);
      return;
    }
    setFresh(true);
    const t = window.setTimeout(() => setFresh(false), left);
    return () => window.clearTimeout(t);
  }, [newest]);
  if (helpers.length === 0) return null;
  const asking = helpers.some(h => h.state === 'question' || h.state === 'waiting');
  const working = helpers.filter(h => h.state === 'working').length;
  const title = [
    `${helpers.length} background helper${helpers.length > 1 ? 's' : ''} of this chat — click to see`,
    ...helpers.map(h => `• ${h.name || h.id}: ${stateLine(h, Date.now())} (${setupLine(h)})`)
  ].join('\n');
  return (
    <span
      className="kids"
      data-asking={asking ? 'yes' : 'no'}
      data-working={working > 0 ? 'yes' : 'no'}
      data-new={fresh ? 'yes' : 'no'}
      title={title}
      onClick={e => {
        e.stopPropagation();
        setAnchor(anchor ? null : (e.currentTarget as HTMLElement));
      }}
    >
      <Icon name="agents" size={12} />
      {helpers.length}
      {anchor && <HelperList helpers={helpers} anchor={anchor} onClose={() => setAnchor(null)} />}
    </span>
  );
}

function HelperList({ helpers, anchor, onClose }: { helpers: BackgroundHelperDto[]; anchor: HTMLElement; onClose: () => void }) {
  const host = useDialogHost();
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const now = useNow(15_000);

  useLayoutEffect(() => {
    const r = anchor.getBoundingClientRect();
    const w = ref.current?.offsetWidth ?? 300;
    const h = ref.current?.offsetHeight ?? 80;
    const left = Math.min(Math.max(8, r.left - 4), window.innerWidth - w - 8);
    const below = r.bottom + 6;
    setPos({ left, top: below + h > window.innerHeight - 8 ? Math.max(8, r.top - h - 6) : below });
  }, [anchor, host, helpers.length]);

  // Any click the list didn't swallow dismisses it, and so does Escape.
  useEffect(() => {
    const close = () => onClose();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    document.addEventListener('click', close);
    window.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('click', close);
      window.removeEventListener('keydown', onKey);
    };
  }, [onClose]);

  const act = (what: string, f: () => Promise<void>) => () => {
    setError(null);
    f().catch(e => {
      logWarn('agents', `${what} failed: ${String(e)}`);
      setError(String(e));
    });
  };

  if (!host) return null;
  return createPortal(
    <div
      ref={ref}
      className="popover"
      onClick={e => e.stopPropagation()}
      style={{ left: pos?.left ?? -9999, top: pos?.top ?? 0, width: 320, display: 'grid', gap: 8 }}
    >
      {helpers.map(h => {
        const line = askLine(h);
        const busy = h.state === 'working' || h.state === 'waiting' || h.state === 'question';
        return (
          <div key={h.id} style={{ display: 'grid', gap: 4 }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 6, minWidth: 0 }}>
              <Icon name={h.provider} size={13} />
              <span style={{ flex: '1 1 auto', fontWeight: 600, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                {h.name || h.id}
              </span>
              <span style={{ flex: 'none', color: 'var(--dim)', fontSize: 'var(--fs-1)' }}>{stateLine(h, now)}</span>
            </div>
            <div style={{ fontSize: 'var(--fs-1)', color: 'var(--dim)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
              {setupLine(h)}
            </div>
            {line && (
              <div style={{ fontSize: 'var(--fs-1)', color: 'var(--dim)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }} title={line}>
                {line}
              </div>
            )}
            <div style={{ display: 'flex', gap: 6 }}>
              <button
                disabled={busy}
                title={busy ? 'Open it once its turn is over' : 'Make it a chat you can see and type in'}
                onClick={act('opening a helper', async () => {
                  await backgroundPromote(h.id);
                  onClose();
                })}
              >
                Open as chat
              </button>
              <button disabled={!busy} onClick={act('stopping a helper', () => backgroundKill(h.id))}>
                Stop
              </button>
              <button onClick={act('deleting a helper', () => backgroundDelete(h.id))}>Delete</button>
            </div>
          </div>
        );
      })}
      {error && <div style={{ color: 'var(--wait)', fontSize: 'var(--fs-1)' }}>{error}</div>}
    </div>,
    host
  );
}
