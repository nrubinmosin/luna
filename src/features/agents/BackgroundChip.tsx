import { useEffect, useState, type CSSProperties, type ReactNode } from 'react';
import { useShallow } from 'zustand/react/shallow';
import { backgroundDelete, backgroundKill, backgroundMoveTargets, type BackgroundHelperDto } from '../../ipc/agents';
import { Icon } from '../../shared/ui/icons';
import { ConfirmDialog } from '../../shared/ui/ConfirmDialog';
import { HoverPopover } from '../../shared/ui/HoverPopover';
import { logWarn } from '../../shared/lib/log';
import { allChats, useChats } from '../chats/chats.store';
import { rootOf, useBackground } from './background.store';
import { openAsChat } from './bridge';
import { HelperView } from './HelperView';
import {
  activityLine,
  askLine,
  asks,
  countHelpers,
  defaultTab,
  helperName,
  helpersIn,
  setupLine,
  stateLine,
  summaryLabel,
  type HelperTab
} from './helperSummary';
import { HelperPill } from './HelperPill';
import { MoveHelperDialog } from './MoveHelperDialog';
import { ownershipLine, suggestedChats } from './ownership';
import { useNameOf } from './useNameOf';
import './helpers.css';

/** How long a chip glows after a helper of its chat was spawned. */
const NEW_MS = 5000;

/** A helper's four buttons (slim, each as wide as its label on one line) share
 *  out its row: at XP's 75px minimum the four would stand past the panel's
 *  edge, and "Open as chat" break onto two lines. */
const ACTION: CSSProperties = { flex: '1 1 auto', whiteSpace: 'nowrap' };

const ONE_LINE: CSSProperties = {
  fontSize: 'var(--fs-1)',
  color: 'var(--dim)',
  overflow: 'hidden',
  textOverflow: 'ellipsis',
  whiteSpace: 'nowrap'
};

/** The time now, again every `ms` while mounted. */
function useNow(ms: number): number {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    const t = window.setInterval(() => setNow(Date.now()), ms);
    return () => window.clearInterval(t);
  }, [ms]);
  return now;
}

/** The helpers hanging from a chat, its helpers' helpers with them. */
export function useHelpersOf(parentId: string): BackgroundHelperDto[] {
  return useBackground(useShallow(s => s.helpers.filter(h => rootOf(h, s.helpers) === parentId)));
}

/**
 * The background helpers of one chat (background/), its helpers' helpers
 * with them: a count on its row, and on hover, focus or a click a list of them
 * with what each is doing, to watch one read-only as it works, open one as a
 * chat of the person's own (which takes it from its agent for good), stop it
 * or delete it. Its agent is the one told what they say; this is for a person
 * who wants to look. Its pane's title bar has the same list (`HelperSummary`).
 */
export function BackgroundChip({ parentId }: { parentId: string }) {
  return <HelperChip helpers={useHelpersOf(parentId)} what="of this chat" />;
}

/**
 * The helpers no chat answers for: the one that started them was deleted, and
 * there is no row of theirs to get at them by. No agent reaches them either,
 * so this is the only way to stop or delete one; left alone, each goes by
 * itself half an hour after it is done (agents/teardown.rs `forget`).
 */
export function LooseHelpers() {
  const chats = useChats(s => s.folders);
  const helpers = useBackground(
    useShallow(s => {
      const rows = new Set(allChats(chats).map(c => c.id));
      return s.helpers.filter(h => {
        const root = rootOf(h, s.helpers);
        return root === null || !rows.has(root);
      });
    })
  );
  if (helpers.length === 0) return null;
  return (
    <div
      className="tp-group"
      title="Background helpers whose chat was deleted. Each is deleted by itself half an hour after it is done."
    >
      <div className="tp-body">
        <div className="chat-row" data-active="no" style={{ cursor: 'default' }}>
          <span style={{ flex: '1 1 auto', paddingLeft: 4, fontSize: 'var(--fs-4)' }}>Helpers with no chat</span>
          <HelperChip helpers={helpers} what="with no chat" />
        </div>
      </div>
    </div>
  );
}

/** A chat's helpers summed up on its pane's title bar: the counts, and the
 *  names that fit after them; the same list as its row's chip. */
export function HelperSummary({ parentId }: { parentId: string }) {
  const helpers = useHelpersOf(parentId);
  const now = useNow(15_000);
  const fresh = useFresh(helpers);
  if (helpers.length === 0) return null;
  const c = countHelpers(helpers);
  return (
    <Helpers
      helpers={helpers}
      what="of this chat"
      faceProps={{
        className: 'hpill',
        title: summaryLabel(helpers, now),
        'data-asking': c.asks > 0 ? 'yes' : 'no',
        'data-new': fresh ? 'yes' : 'no'
      }}
      face={<HelperPill helpers={helpers} now={now} />}
    />
  );
}

/** Whether a helper of these was spawned within NEW_MS: the one moment nothing
 *  else on screen marks, so the face glows for a while. */
function useFresh(helpers: BackgroundHelperDto[]): boolean {
  const newest = Math.max(0, ...helpers.map(h => h.startedMs));
  const [fresh, setFresh] = useState(false);
  useEffect(() => {
    const left = newest + NEW_MS - Date.now();
    // Also when the newest helper went inside its glow: the cleanup took the
    // timer that would have put the glow out.
    if (left <= 0) {
      // eslint-disable-next-line react-hooks/set-state-in-effect -- the glow runs on the clock, outside React: it is set as the timer is armed or skipped
      setFresh(false);
      return;
    }
    setFresh(true);
    const t = window.setTimeout(() => setFresh(false), left);
    return () => window.clearTimeout(t);
  }, [newest]);
  return fresh;
}

function HelperChip({ helpers, what }: { helpers: BackgroundHelperDto[]; what: string }) {
  const fresh = useFresh(helpers);
  if (helpers.length === 0) return null;
  const c = countHelpers(helpers);
  return (
    <Helpers
      helpers={helpers}
      what={what}
      faceProps={{
        className: 'kids',
        // eslint-disable-next-line react-hooks/purity -- the tooltip gives ages as of this render, which comes with every poll
        title: summaryLabel(helpers, Date.now()),
        'data-asking': c.asks > 0 ? 'yes' : 'no',
        'data-working': c.working > 0 ? 'yes' : 'no',
        'data-new': fresh ? 'yes' : 'no'
      }}
      face={
        <>
          <Icon name="agents" size={12} />
          {helpers.length}
        </>
      }
    />
  );
}

/** A face that opens the helpers' list, with what the list opens: a helper
 *  watched, the question before opening one as a chat, or where to move one.
 *  Kept here, not in the list: the list closes as they open, and they go on. */
function Helpers({
  helpers,
  what,
  face,
  faceProps
}: {
  helpers: BackgroundHelperDto[];
  what: string;
  face: ReactNode;
  faceProps: Parameters<typeof HoverPopover>[0]['faceProps'];
}) {
  const [watching, setWatching] = useState<string | null>(null);
  const [handing, setHanding] = useState<BackgroundHelperDto | null>(null);
  const [moving, setMoving] = useState<{ helper: BackgroundHelperDto; to?: string } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const nameOf = useNameOf();
  const watched = helpers.find(h => h.id === watching);
  return (
    <>
      <HoverPopover
        face={face}
        faceProps={faceProps}
        label={`Background helpers ${what}`}
        panelClassName="helper-panel"
      >
        {close => (
          <HelperPanel
            helpers={helpers}
            onWatch={id => {
              setWatching(id);
              close();
            }}
            onOpen={h => {
              setHanding(h);
              close();
            }}
            onMove={(helper, to) => {
              setMoving({ helper, to });
              close();
            }}
            error={error}
            setError={setError}
          />
        )}
      </HoverPopover>
      {watched && <HelperView helper={watched} onClose={() => setWatching(null)} />}
      {moving && (
        <MoveHelperDialog helper={moving.helper} prefer={moving.to} nameOf={nameOf} onClose={() => setMoving(null)} />
      )}
      {handing && (
        <ConfirmDialog
          title={`Open "${helperName(handing)}" as your chat?`}
          body={
            <>
              It becomes a chat of your own, for good: its agent can no longer send to it, wait on it, stop or delete
              it, and is told so once. Anything still queued for it is not delivered; whoever sent it hears where it is.
            </>
          }
          confirmLabel="Open as chat"
          danger={false}
          onCancel={() => setHanding(null)}
          onConfirm={() => {
            const h = handing;
            setHanding(null);
            setError(null);
            openAsChat(h.id).catch(e => {
              logWarn('agents', `opening a helper failed: ${String(e)}`);
              setError(String(e));
            });
          }}
        />
      )}
    </>
  );
}

/** For each helper that chats were refused a write into, the chats it may move
 *  to now (agents/reparent.rs `move_targets`): the row offers only those, as
 *  the core would refuse the rest. Asked again as the helpers or the chats that
 *  asked change. */
function useMoveOffers(helpers: BackgroundHelperDto[]): Record<string, ReadonlySet<string>> {
  const asked = helpers
    .filter(h => h.suggested?.length)
    .map(h => `${h.id} ${suggestedChats(h).join(' ')}`)
    .join('\n');
  const [offers, setOffers] = useState<Record<string, ReadonlySet<string>>>({});
  useEffect(() => {
    if (!asked) return;
    let live = true;
    void Promise.all(
      asked.split('\n').map(line => {
        const [id = ''] = line.split(' ');
        return backgroundMoveTargets(id).then(
          (t): [string, ReadonlySet<string>] => [
            id,
            new Set(t.blocked ? [] : t.targets.filter(x => x.ok).map(x => x.id))
          ],
          (e): [string, ReadonlySet<string>] => {
            logWarn('agents', `listing where a helper may move failed: ${String(e)}`);
            return [id, new Set()];
          }
        );
      })
    ).then(rows => {
      if (live) setOffers(Object.fromEntries(rows));
    });
    return () => {
      live = false;
    };
  }, [asked]);
  return offers;
}

/** The list itself: Working and Idle tabs over the rows, which scroll under them. */
export function HelperPanel({
  helpers,
  onWatch,
  onOpen,
  onMove,
  error,
  setError
}: {
  helpers: BackgroundHelperDto[];
  onWatch: (id: string) => void;
  onOpen: (h: BackgroundHelperDto) => void;
  /** Opens the Move dialog, on chat `to` if the row offered it. */
  onMove: (h: BackgroundHelperDto, to?: string) => void;
  error: string | null;
  setError: (e: string | null) => void;
}) {
  const now = useNow(15_000);
  const nameOf = useNameOf();
  const listed = new Set(helpers.map(h => h.id));
  const offers = useMoveOffers(helpers.filter(h => !(h.parent && listed.has(h.parent))));
  // The tab picked, if any; else whichever has work in it, as helpers come and go.
  const [picked, setPicked] = useState<HelperTab | null>(null);
  const tab = picked ?? defaultTab(helpers);
  const working = helpersIn(helpers, 'working');
  const idle = helpersIn(helpers, 'idle');
  const shown = tab === 'working' ? working : idle;

  const act = (what: string, f: () => Promise<void>) => () => {
    setError(null);
    f().catch(e => {
      logWarn('agents', `${what} failed: ${String(e)}`);
      setError(String(e));
    });
  };

  const tabButton = (key: HelperTab, label: string, n: number) => (
    <button
      role="tab"
      className="slim"
      aria-selected={tab === key}
      data-on={tab === key ? 'yes' : 'no'}
      onClick={() => setPicked(key)}
    >
      {label} {n}
    </button>
  );

  return (
    <>
      <div className="helper-tabs" role="tablist" aria-label="Helpers by state">
        {tabButton('working', 'Working', working.length)}
        {tabButton('idle', 'Idle', idle.length)}
      </div>
      <div className="helper-rows" role="tabpanel">
        {shown.length === 0 && (
          <div style={{ ...ONE_LINE, padding: '4px 2px' }}>{tab === 'working' ? 'None at work.' : 'None idle.'}</div>
        )}
        {shown.map(h => {
          const line = askLine(h);
          const asking = asks(h);
          const busy = h.state === 'working' || h.state === 'starting' || asking;
          // Between turns, what its turns left running is what there is to stop.
          const left = h.state === 'background' ? (h.leftRunning ?? []) : [];
          const activity = activityLine(h, now);
          const marks = ownershipLine(h, nameOf, now);
          // A helper's helper moves only with the chat's own helper above it (agents/reparent.rs `check_source`).
          const under = h.parent && listed.has(h.parent) ? h.parent : null;
          return (
            <div key={h.id} className="helper-row" data-helper={h.id} data-asking={asking ? 'yes' : 'no'}>
              <div style={{ display: 'flex', alignItems: 'center', gap: 6, minWidth: 0 }}>
                {asking ? <b className="ask-mark" aria-label="asks" /> : <Icon name={h.provider} size={13} />}
                <span
                  title={helperName(h)}
                  style={{
                    flex: '1 1 auto',
                    fontWeight: 600,
                    overflow: 'hidden',
                    textOverflow: 'ellipsis',
                    whiteSpace: 'nowrap'
                  }}
                >
                  {helperName(h)}
                </span>
                <span style={{ flex: 'none', color: 'var(--dim)', fontSize: 'var(--fs-1)' }}>{stateLine(h, now)}</span>
              </div>
              <div style={ONE_LINE}>{[setupLine(h), activity].filter(Boolean).join(' · ')}</div>
              {h.task && (
                <div style={{ ...ONE_LINE, color: 'var(--fg)' }} title={h.task}>
                  {h.task}
                </div>
              )}
              {line && (
                <div className="helper-ask" title={line}>
                  {line}
                </div>
              )}
              {/* Whose it is, and the way to make it another chat's: a link on
                  that line, not a fifth button, which the row has no room for. */}
              <div style={{ display: 'flex', alignItems: 'baseline', gap: 6, minWidth: 0 }}>
                <span style={{ ...ONE_LINE, flex: '1 1 auto' }} title={marks} data-testid="helper-marks">
                  {marks}
                </span>
                <button
                  className="helper-link"
                  style={{ flex: 'none' }}
                  disabled={!!under}
                  title={
                    under
                      ? `It moves with "${nameOf(under)}", the helper above it`
                      : 'Move it, with the helpers under it, to another of your chats'
                  }
                  onClick={() => onMove(h)}
                >
                  Move to chat…
                </button>
              </div>
              {/* Only a chat it may go to now; a click opens the Move dialog on
                  it, so a link that shows up under the pointer moves nothing. */}
              {!under &&
                suggestedChats(h)
                  .filter(chat => offers[h.id]?.has(chat))
                  .map(chat => (
                    <button
                      key={chat}
                      className="helper-link"
                      title={`"${nameOf(chat)}" tried to send to it or answer it and was refused, not being above it. Move it there, with the helpers under it?`}
                      onClick={() => onMove(h, chat)}
                    >
                      Move to {nameOf(chat)}?
                    </button>
                  ))}
              <div style={{ display: 'flex', gap: 6 }}>
                <button
                  className="slim"
                  style={ACTION}
                  title="Its conversation as it goes, read-only"
                  onClick={() => onWatch(h.id)}
                >
                  Watch
                </button>
                <button
                  className="slim"
                  style={ACTION}
                  disabled={busy || left.length > 0}
                  title={
                    busy
                      ? 'Open it once its turn is over'
                      : left.length > 0
                        ? 'Stop what its turn left running first'
                        : 'Make it a chat of your own; its agent loses it for good'
                  }
                  onClick={() => onOpen(h)}
                >
                  Open as chat
                </button>
                <button
                  className="slim"
                  style={ACTION}
                  disabled={!busy && left.length === 0}
                  title={left.length > 0 ? `End what its turn left running: ${left.join(', ')}` : undefined}
                  onClick={act('stopping a helper', () => backgroundKill(h.id))}
                >
                  Stop
                </button>
                <button
                  className="slim"
                  style={ACTION}
                  disabled={busy || left.length > 0}
                  title={
                    busy || left.length > 0 ? 'Stop it first: it is at work' : 'Delete it, and the worktree it made'
                  }
                  onClick={act('deleting a helper', async () => {
                    const r = await backgroundDelete(h.id);
                    // Its worktree held work saved nowhere else: kept, and said.
                    if (r.worktreeKept)
                      setError(`Deleted; its worktree was kept for its uncommitted work: ${r.worktreeKept}`);
                  })}
                >
                  Delete
                </button>
              </div>
            </div>
          );
        })}
      </div>
      {error && <div style={{ color: 'var(--wait)', fontSize: 'var(--fs-1)', flex: 'none' }}>{error}</div>}
    </>
  );
}
