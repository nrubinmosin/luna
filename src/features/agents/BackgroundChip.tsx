import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { useShallow } from 'zustand/react/shallow';
import { backgroundDelete, backgroundKill, backgroundPromote, type BackgroundHelperDto } from '../../ipc/commands';
import { Icon } from '../../shared/ui/icons';
import { useDialogHost } from '../../shared/ui/dialogHost';
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
  if (helpers.length === 0) return null;
  const asking = helpers.some(h => h.state === 'question' || h.state === 'waiting');
  const working = helpers.filter(h => h.state === 'working').length;
  const title = [
    `${helpers.length} background helper${helpers.length > 1 ? 's' : ''} of this chat — click to see`,
    ...helpers.map(h => `• ${h.name || h.id}: ${STATE_LABEL[h.state] ?? h.state}`)
  ].join('\n');
  return (
    <span
      className="kids"
      data-asking={asking ? 'yes' : 'no'}
      data-working={working > 0 ? 'yes' : 'no'}
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
              <span style={{ flex: 'none', color: 'var(--dim)', fontSize: 'var(--fs-1)' }}>{STATE_LABEL[h.state] ?? h.state}</span>
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
