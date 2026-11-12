/**
 * "Move to another account…": the chat goes on as a new chat on another
 * account of its CLI, from a document the person reads, may edit, and sends
 * (docs/adr/0023-moving-a-chat-to-another-account.md). Two steps: what goes
 * where (the account, who writes the document, the old chat kept or stopped,
 * a note), then the whole of what goes, the line Luna types and the
 * document, with Send bound to exactly that text.
 */
import { useEffect, useId, useMemo, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import {
  moveAsk,
  moveBuildFromRecords,
  moveCancel,
  moveConfirm,
  moveFacts,
  moveList,
  movePrepare,
  movePreview,
  sha256Hex,
  type LimitFact,
  type MoveFactsDto,
  type MovePhase,
  type MovePreviewDto
} from '../../ipc/moving';
import { fmtReset, fmtResetDate } from '../../shared/lib/format';
import { accountKey, PROVIDER_LABEL, type Account, type Chat } from '../../shared/types';
import { useModal } from '../../shared/ui/modal';
import { useAccounts } from '../accounts/accounts.store';
import { AccountPicker } from '../new-chat/AccountPicker';
import { ui } from '../providers';
import { useChats, newId } from './chats.store';
import { carryOut, launchFor, modelFor } from './moving';

/** One XP.css choice: the control, then its label by `for`, which is what
 *  XP.css draws the box or the dot on. */
function Opt({
  id,
  type,
  name,
  checked,
  disabled,
  onChange,
  title,
  children
}: {
  id: string;
  type: 'radio' | 'checkbox';
  name?: string;
  checked: boolean;
  disabled?: boolean;
  onChange: () => void;
  title?: string;
  children: ReactNode;
}) {
  return (
    <div className="move-opt" title={title}>
      <input type={type} id={id} name={name} checked={checked} disabled={disabled} onChange={onChange} />
      <label htmlFor={id}>{children}</label>
    </div>
  );
}

/** How often the dialog looks whether the chat has written its document. */
const ASKED_EVERY_MS = 2000;
/** How often the setup looks again at what keeps the chat from being asked:
 *  a question answered in its pane meanwhile clears it. */
const FACTS_EVERY_MS = 3000;

/** The first account that is not this chat's, of its CLI, signed in, with
 *  every window under 95%; else the first signed in; else the first. */
export function defaultTarget(accounts: Account[], chat: Chat): Account | null {
  const others = accounts.filter(a => a.provider === chat.provider && a.name !== chat.account);
  const signedIn = others.filter(a => a.signedIn);
  return signedIn.find(a => a.haveUsage && a.limits.every(l => l.used < 0.95)) ?? signedIn[0] ?? others[0] ?? null;
}

/** Why the chat is not to write its own document now, with what clears the
 *  way, and whether that keeps the choice from being made: a question of its
 *  CLI's does not, as the person answers it in the pane, the dialog open. */
export interface AskRefusal {
  why: string;
  blocks: boolean;
}

const sentence = (s: string) => {
  const t = s.charAt(0).toUpperCase() + s.slice(1);
  return /[.!?]$/.test(t) ? t : `${t}.`;
};
const lower = (s: string) => s.charAt(0).toLowerCase() + s.slice(1).replace(/\.$/, '');

/**
 * Why "The chat writes it" is not offered, or null where it is: a chat with
 * Luna's tools whose CLI has listed the handoff tool, as the core says in
 * `blocked` (move_facts `askBlocked`). What the window knows of the chat is
 * said first; the core's words are turned into what to do where they are
 * known, and passed on as they are where not.
 */
export function askRefusal(chat: Chat, blocked: string | null | undefined): AskRefusal | null {
  if (!chat.tools)
    return {
      why: "It runs without Luna's tools, so it has no handoff tool to send its document with: start a chat with the Luna tools box ticked.",
      blocks: true
    };
  if (chat.provider === 'claude' && chat.perm === 'Plan only')
    return { why: 'In plan mode the chat writes no file: leave plan mode first.', blocks: true };
  if (chat.provider === 'codex' && (chat.sandbox === 'read-only' || chat.sandbox === 'read-only-net'))
    return { why: "Codex's read-only sandbox writes no file: start the chat with workspace-write.", blocks: true };
  if (!blocked) return null;
  // A chat at rest may well be running: whether its CLI runs is the core's to
  // say.
  if (/not running/.test(blocked))
    return { why: 'Its CLI is not running: start the chat in its pane, then ask it.', blocks: true };
  if (/waiting on a question/.test(blocked))
    return { why: "Its CLI asks you something: answer its CLI's question in its pane, then ask it.", blocks: false };
  if (/usage limit/.test(blocked)) return { why: 'It stopped on a usage limit and cannot write now.', blocks: true };
  return { why: sentence(blocked), blocks: true };
}

const clock = (ms: number) => new Date(ms).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });

/** Why Luna's document is preselected, said of a usage limit in force. */
export function limitReason(l: LimitFact): string {
  const kind = l.kind === 'usage_limit' ? '' : ` (${l.kind.replace(/_/g, ' ')})`;
  return `the chat stopped on a usage limit${kind} at ${clock(l.since)}`;
}

/** What is known of the window a usage limit filled: which, how full, when it
 *  resets, and how old the reading is; null when nothing is. */
export function quotaWords(l: LimitFact, now = Date.now()): string | null {
  const parts: string[] = [];
  const used = l.used == null ? '' : `${Math.round(l.used * 100)}%`;
  if (l.window || used) parts.push([l.window, used && `at ${used}`].filter(Boolean).join(' '));
  if (l.resetsAt) parts.push(`resets ${fmtResetDate(l.resetsAt) ?? l.resetsAt} (in ${fmtReset(l.resetsAt, now)})`);
  if (l.readingAgeMs != null) {
    const m = Math.round(l.readingAgeMs / 60_000);
    parts.push(`usage read ${m < 1 ? 'just now' : `${m} min ago`}`);
  }
  return parts.length ? parts.join(' · ') : null;
}

type Step =
  | { at: 'setup' }
  | { at: 'asking'; moveId: string; newId: string }
  | { at: 'failed'; moveId: string; why: string; askAgain: boolean }
  | Shown
  | { at: 'sending'; shown: Shown };

/** The preview, with who wrote what it shows and, for Luna's, why hers. */
interface Shown {
  at: 'preview';
  preview: MovePreviewDto;
  text: string;
  by: 'chat' | 'luna';
  why?: string;
}

export function MoveChatDialog({ chat, onClose }: { chat: Chat; onClose: () => void }) {
  const accounts = useAccounts(s => s.accounts);
  const folder = useChats(s => s.folderOf(chat.id));
  const folders = useChats(s => s.folders);
  const usedAccounts = useMemo(
    () => new Set(folders.flatMap(f => f.chats.map(c => accountKey(c.provider, c.account)))),
    [folders]
  );
  const targets = useMemo(
    () => accounts.filter(a => a.provider === chat.provider && a.name !== chat.account),
    [accounts, chat.provider, chat.account]
  );
  const [target, setTarget] = useState(() => {
    const t = defaultTarget(accounts, chat);
    return t ? accountKey(t.provider, t.name) : '';
  });
  const to = targets.find(a => accountKey(a.provider, a.name) === target) ?? null;
  // In a Codex worktree, or started into one before its first turn says where.
  const codexWorktree = chat.provider === 'codex' && (chat.worktree || !!chat.worktreePath);
  const claudeWorktree = chat.provider === 'claude' && chat.worktree;
  // What the core says before a move: a usage limit in force, and what keeps
  // the chat from being asked. Null until it has answered.
  const [facts, setFacts] = useState<MoveFactsDto | null>(null);
  const limit = facts?.limit ?? null;
  const refusal = askRefusal(chat, facts?.askBlocked);
  // The chat writes it unless the person picks otherwise; Luna's from records
  // first only where a usage limit holds the chat, or it cannot be asked.
  const [picked, setPicked] = useState<'luna' | 'chat' | null>(null);
  const source = picked ?? (limit || refusal?.blocks ? 'luna' : 'chat');
  const [stopOld, setStopOld] = useState(claudeWorktree);
  const [note, setNote] = useState('');
  const [step, setStep] = useState<Step>({ at: 'setup' });
  const [phase, setPhase] = useState<MovePhase>('asking');
  // The last move of this chat, when it ended with the chat asked and no
  // document: said again on opening the dialog.
  const [lastFailed, setLastFailed] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [model, setModel] = useState<{ model: string | null; effort: string; kept: boolean } | null>(null);
  const [trusted, setTrusted] = useState<boolean | null>(null);
  const [trustIt, setTrustIt] = useState(false);
  const moveRef = useRef<{ moveId: string } | null>(null);
  // Closed while the core was still answering: the move it then names is
  // given up as soon as its id is known, never left asking unseen.
  const closedRef = useRef(false);
  useEffect(() => {
    // Mounted again (React's strict mode does so at once): open again.
    closedRef.current = false;
    return () => {
      closedRef.current = true;
      const m = moveRef.current;
      if (m) void moveCancel(chat.id, m.moveId).catch(() => {});
    };
  }, [chat.id]);

  // What the new chat starts with on the account picked: its model, and
  // whether the folder is trusted there.
  useEffect(() => {
    let stale = false;
    // eslint-disable-next-line react-hooks/set-state-in-effect -- another account's answers start unknown
    setModel(null);
    setTrusted(null);
    setTrustIt(false);
    if (!to || !folder) return;
    void modelFor(chat, to, folder.path)
      .then(m => !stale && setModel(m))
      .catch(() => !stale && setModel({ model: chat.model, effort: chat.effort, kept: true }));
    void ui(chat.provider)
      .folderTrusted(to.path, folder.path)
      .then(t => !stale && setTrusted(t))
      .catch(() => !stale && setTrusted(false));
    return () => {
      stale = true;
    };
  }, [to, folder, chat]);

  // The last move of this chat, once, as the dialog opens.
  useEffect(() => {
    let stale = false;
    void moveList()
      .then(moves => {
        const last = moves.filter(m => m.chatId === chat.id).sort((a, b) => b.at - a.at)[0];
        if (!stale && last?.askFailed) setLastFailed(last.why ?? 'no reason was given');
      })
      .catch(() => {});
    return () => {
      stale = true;
    };
  }, [chat.id]);

  // What the core says of the chat, looked at again while the person picks.
  const settingUp = step.at === 'setup';
  useEffect(() => {
    if (!settingUp) return;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const look = async () => {
      const f = await moveFacts(chat.id).catch((): MoveFactsDto => ({}));
      if (stopped) return;
      // Nothing at all from a window with no core (a plain browser): no facts.
      setFacts(f ?? {});
      timer = setTimeout(() => void look(), FACTS_EVERY_MS);
    };
    void look();
    return () => {
      stopped = true;
      if (timer) clearTimeout(timer);
    };
  }, [settingUp, chat.id]);

  // A chat asked for its document: looked at until it is in, or the ask ends.
  // A failed ask stays with why: Luna's document is the person's to choose.
  const askingId = step.at === 'asking' ? step.moveId : null;
  useEffect(() => {
    if (!askingId) return;
    let stopped = false;
    const look = async () => {
      const m = (await moveList().catch(() => [])).find(x => x.moveId === askingId);
      if (stopped) return;
      if (m) setPhase(m.phase);
      if (m?.phase === 'shown') {
        const preview = await movePreview(chat.id, askingId).catch((e: unknown) => {
          setError(String(e));
          return null;
        });
        if (preview && !stopped) setStep({ at: 'preview', preview, text: preview.text, by: 'chat' });
        return;
      }
      if (m?.phase === 'failed' && m.askFailed) {
        setStep({
          at: 'failed',
          moveId: askingId,
          why: m.why ?? 'no reason was given',
          askAgain: !!m.askAgain
        });
        return;
      }
      if (m && (m.phase === 'failed' || m.phase === 'canceled')) {
        moveRef.current = null;
        setError(m.why ?? 'The move ended before the chat wrote its document.');
        setStep({ at: 'setup' });
        return;
      }
      timer = setTimeout(() => void look(), ASKED_EVERY_MS);
    };
    let timer = setTimeout(() => void look(), ASKED_EVERY_MS);
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  }, [askingId, chat.id]);

  const cancel = () => {
    const m = moveRef.current;
    if (m && step.at !== 'sending') void moveCancel(chat.id, m.moveId).catch(() => {});
    moveRef.current = null;
    closedRef.current = true;
    onClose();
  };

  /** A move the core began for this dialog: kept, or given up at once where
   *  the dialog has closed meanwhile. Whether it is to go on. */
  const began = (moveId: string) => {
    if (closedRef.current) {
      void moveCancel(chat.id, moveId).catch(() => {});
      return false;
    }
    moveRef.current = { moveId };
    return true;
  };

  const trustNeeded = trusted === false && claudeWorktree && !trustIt;
  // A signed-out account would meet the new chat with a sign-in, not a
  // prompt: refused until it is signed in.
  const signedOut = !!to && !to.signedIn;
  const canGo =
    !!to && !signedOut && !!folder && !!model && trusted !== null && !trustNeeded && !codexWorktree && !busy;

  // Why Luna writes it, as the preview's header says.
  const lunaWhy = limit
    ? limitReason(limit)
    : picked !== 'luna' && refusal?.blocks
      ? `the chat could not be asked: ${lower(refusal.why)}`
      : 'as you chose';

  const build = async (from: 'luna' | 'chat') => {
    if (!to || !folder) return;
    setBusy(true);
    setError(null);
    const id = newId('c');
    try {
      if (from === 'chat') {
        const s = await moveAsk(chat.id, id, to.path, note);
        if (began(s.moveId)) {
          setPhase(s.phase);
          setStep({ at: 'asking', moveId: s.moveId, newId: id });
        }
      } else {
        const preview = await movePrepare(chat.id, id, to.path, chat.sessionId ?? null, note);
        if (began(preview.moveId)) setStep({ at: 'preview', preview, text: preview.text, by: 'luna', why: lunaWhy });
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  /** Luna's document for a move whose chat was asked: the ask closed, the
   *  account, the note and the choices made kept. */
  const fromRecords = async (moveId: string, why: string) => {
    setBusy(true);
    setError(null);
    try {
      const preview = await moveBuildFromRecords(chat.id, moveId, chat.sessionId ?? null);
      if (closedRef.current) return;
      moveRef.current = { moveId: preview.moveId };
      setStep({ at: 'preview', preview, text: preview.text, by: 'luna', why });
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  // Taken before anything awaits: two clicks in one task see the same
  // rendered step, and only one Send may go.
  const sendingRef = useRef(false);
  const send = async () => {
    if (sendingRef.current || step.at !== 'preview' || !to || !folder || !model) return;
    sendingRef.current = true;
    const shown = step;
    const { preview, text } = shown;
    setStep({ at: 'sending', shown });
    setError(null);
    try {
      if (trustIt && trusted === false) await ui(chat.provider).trustFolder(to.path, folder.path);
      const launch = launchFor(chat, folder.path, to, model.model, model.effort, stopOld || preview.stopOld);
      const status = await moveConfirm(chat.id, preview.moveId, text, await sha256Hex(text), launch);
      moveRef.current = null;
      await carryOut(status);
      onClose();
    } catch (e) {
      setError(String(e));
      setStep(shown);
    } finally {
      sendingRef.current = false;
    }
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // An open account list takes its own Escape.
      if (e.key === 'Escape' && !document.querySelector('[data-picker-open]')) cancel();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });

  const host = document.querySelector('[data-app]') ?? document.body;
  const box = useRef<HTMLDivElement>(null);
  // While the chat is asked the person may have to answer it in its pane:
  // the dialog is then a banner, nothing else made inert.
  const trap = useModal(box, step.at === 'asking' ? null : host);
  const titleId = useId();
  const uid = useId();
  const label = PROVIDER_LABEL[chat.provider];
  const bytes = step.at === 'preview' ? new TextEncoder().encode(step.text).length : 0;

  if (step.at === 'asking') {
    // Its CLI asking the person something comes first: nothing goes on
    // until it is answered in the pane.
    const words = error
      ? error
      : chat.status === 'waiting'
        ? `"${chat.name}"'s CLI asks you something: answer it in its pane`
        : phase === 'asked'
          ? `"${chat.name}" is writing its document`
          : `Waiting for "${chat.name}"'s turn to end, then Luna asks it for its document`;
    return createPortal(
      <div className="window move-banner" role="status" data-testid="move-banner">
        <div className="window-body">
          <span
            data-testid="move-banner-words"
            title={`${words}. Nothing is cleared there. If its CLI asks to create the file, answer in its pane.`}
          >
            {words}
          </span>
          <button
            disabled={busy}
            title="Closes the ask, so a document the chat sends late is refused, and builds Luna's from the chat's records for you to read before Send"
            onClick={() => void fromRecords(step.moveId, 'you did not wait for the chat')}
          >
            Use Luna's document now
          </button>
          <button onClick={cancel}>Cancel the move</button>
        </div>
      </div>,
      host
    );
  }

  return createPortal(
    <div
      className="scrim"
      onClick={e => {
        e.stopPropagation();
        if (step.at === 'setup') cancel();
      }}
      style={{
        position: 'fixed',
        inset: 0,
        background: 'rgba(0,0,0,.35)',
        display: 'grid',
        placeItems: 'center',
        zIndex: 80
      }}
    >
      <div
        ref={box}
        onClick={e => e.stopPropagation()}
        onKeyDown={trap}
        className="window move-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        data-testid="move-dialog"
        style={{
          width: step.at === 'preview' ? 'min(760px, 96vw)' : 'min(460px, 96vw)',
          maxHeight: '94vh',
          display: 'flex',
          flexDirection: 'column',
          outline: 'none',
          boxShadow: 'var(--shadow), var(--border-window-outer), var(--border-window-inner)'
        }}
      >
        <div className="title-bar">
          <div className="title-bar-text" id={titleId}>
            Move "{chat.name.length > 34 ? `${chat.name.slice(0, 34)}…` : chat.name}" to another account
          </div>
          <div className="title-bar-controls">
            <button aria-label="Close" onClick={cancel} />
          </div>
        </div>
        <div className="window-body move-body">
          {codexWorktree ? (
            <p>
              This Codex chat works in a worktree Codex keeps in its account's folder, beside that account's
              credentials. A chat of another account is not to work there, so it does not move. Start a chat on the
              other account in the project folder instead.
            </p>
          ) : step.at === 'setup' ? (
            <>
              <p className="move-lead">
                A new {label} chat goes on from a document on the account you pick. The conversation itself stays here.
              </p>
              {targets.length === 0 ? (
                <p className="move-warn">No other {label} account to move to.</p>
              ) : (
                <label className="move-row">
                  <span>Account</span>
                  <AccountPicker accounts={targets} value={target} onChange={setTarget} />
                </label>
              )}
              {signedOut && (
                <div className="move-warn" role="alert">
                  {to?.name} is signed out: sign in to it in Accounts first, then move the chat there.
                </div>
              )}
              {to && (
                <div className="move-facts">
                  <div>
                    Model{' '}
                    {model ? (
                      <>
                        <b>{model.model ?? 'default'}</b>
                        {model.effort && <> · effort {model.effort}</>}
                        {!model.kept && <> (the account's default: it does not list this chat's)</>}
                      </>
                    ) : (
                      '…'
                    )}
                  </div>
                  <div>
                    Permissions as this chat has them:{' '}
                    <b>{chat.provider === 'claude' ? chat.perm : `${chat.approval}, ${chat.sandbox}`}</b>
                  </div>
                  {!usedAccounts.has(accountKey(to.provider, to.name)) && (
                    <div className="move-warn">
                      No chat here used this account yet: its first-run screens may show in the new chat's pane. Luna
                      waits for them before it types.
                    </div>
                  )}
                </div>
              )}
              {to && trusted === false && (
                <Opt id={`${uid}-trust`} type="checkbox" checked={trustIt} onChange={() => setTrustIt(!trustIt)}>
                  Trust {folder?.path} on {to.name}
                  {claudeWorktree ? ' (needed: in a worktree Claude Code cannot ask)' : ''}
                </Opt>
              )}
              <div className="move-group" role="group" aria-labelledby={`${uid}-doc`}>
                <div className="move-label" id={`${uid}-doc`}>
                  The document
                </div>
                <Opt
                  id={`${uid}-chat`}
                  type="radio"
                  name="move-source"
                  disabled={!!refusal?.blocks}
                  checked={source === 'chat'}
                  onChange={() => setPicked('chat')}
                  title={
                    refusal?.why ??
                    'The chat writes it with what it knows, in one more turn over its whole context; nothing is cleared there, and you read it before Send'
                  }
                >
                  The chat writes it{refusal ? ` — ${refusal.why}` : ''}
                </Opt>
                <Opt
                  id={`${uid}-luna`}
                  type="radio"
                  name="move-source"
                  checked={source === 'luna'}
                  onChange={() => setPicked('luna')}
                  title="Luna builds it from the chat's transcript and the working folder: no turn of the chat's"
                >
                  Luna builds it from the chat's records
                </Opt>
                {limit && (
                  <div className="move-warn" data-testid="move-limit">
                    {sentence(limitReason(limit))}: Luna's document is picked, as the chat cannot write now.
                  </div>
                )}
                {limit && quotaWords(limit) && (
                  <div className="move-facts" data-testid="move-quota">
                    {quotaWords(limit)}
                  </div>
                )}
                {lastFailed && (
                  <div className="move-warn" data-testid="move-last-failed">
                    Last time the chat did not write its document: {lastFailed}
                  </div>
                )}
              </div>
              <div className="move-group" role="group" aria-labelledby={`${uid}-old`}>
                <div className="move-label" id={`${uid}-old`}>
                  This chat
                </div>
                <Opt
                  id={`${uid}-keep`}
                  type="radio"
                  name="move-old"
                  disabled={claudeWorktree}
                  checked={!stopOld}
                  onChange={() => setStopOld(false)}
                >
                  Keep it
                </Opt>
                <Opt
                  id={`${uid}-stop`}
                  type="radio"
                  name="move-old"
                  checked={stopOld}
                  onChange={() => setStopOld(true)}
                >
                  Stop it
                  {claudeWorktree ? ' (always, in a Claude Code worktree: the new chat goes into it)' : ''}
                </Opt>
                {!stopOld && <div className="move-warn">Kept, both chats work in one folder.</div>}
                {stopOld && chat.status === 'background' && (
                  <div className="move-warn">Its subagents and jobs still at work stop with it.</div>
                )}
              </div>{' '}
              <label className="move-row move-note">
                <span>Your instruction to the new chat (optional): typed into it as yours, outside the document</span>
                <textarea
                  rows={2}
                  value={note}
                  placeholder="What the new chat should do first"
                  onChange={e => setNote(e.target.value)}
                />
              </label>
            </>
          ) : step.at === 'failed' ? (
            <>
              <p className="move-lead" data-testid="move-failed">
                The chat did not write its document: {step.why}
              </p>
              <p className="move-facts">
                Luna can build it from the chat's records instead, for <b>{to?.name}</b>, with your instruction and
                choices as they were. You read it before Send.
              </p>
            </>
          ) : step.at === 'preview' ? (
            <>
              <div className="move-source" data-testid="move-source">
                {step.by === 'chat'
                  ? 'Written by the chat'
                  : `Built by Luna from the chat's records — ${step.why ?? 'as you chose'}`}
              </div>
              <p className="move-lead">
                Everything below goes to {label} account <b>{to?.name}</b>. Read it; edit the document if you like.
              </p>
              <div className="move-counts">
                Luna took out {step.preview.redacted} secret-like string(s), {step.preview.pathsCut} folder path(s) of
                accounts or Luna's, and {step.preview.invisible} hidden character(s). This is no guarantee the text is
                clean.
              </div>
              {step.preview.highlights.length > 0 && (
                <div className="move-warn">
                  Look at these before you send: {step.preview.highlights.map(h => `${h.slice(0, 12)}…`).join(', ')}
                </div>
              )}
              {step.preview.warnings.map(w => (
                <div key={w} className="move-warn">
                  {w}
                </div>
              ))}
              <div className="move-label">The line Luna types into the new chat</div>
              <pre className="move-line">{step.preview.prompt}</pre>
              <div className="move-label">
                The document ({Math.ceil(bytes / 1024)} of {step.preview.maxBytes / 1024} KB)
              </div>
              <textarea
                className="move-doc"
                aria-label="The document"
                spellCheck={false}
                value={step.text}
                onChange={e => setStep({ ...step, text: e.target.value })}
              />
            </>
          ) : (
            <p>Sending…</p>
          )}
          {error && (
            <div className="move-error" role="alert">
              {error}
            </div>
          )}
        </div>
        <div className="dialog-foot">
          <button onClick={cancel}>Cancel</button>
          {step.at === 'setup' && !codexWorktree && (
            <button
              className="primary"
              disabled={!canGo || !facts || (source === 'chat' && !!refusal?.blocks)}
              onClick={() => void build(source)}
            >
              {source === 'chat' ? 'Ask the chat' : 'Preview'}
            </button>
          )}
          {step.at === 'failed' && (
            <>
              {step.askAgain && (
                <button
                  disabled={busy || !canGo}
                  title="Asks the chat once more: another turn over its whole context, on its account"
                  onClick={() => void build('chat')}
                >
                  Ask again
                </button>
              )}
              <button
                className="primary"
                disabled={busy}
                onClick={() => void fromRecords(step.moveId, `the chat did not write its document: ${step.why}`)}
              >
                Build from records
              </button>
            </>
          )}
          {step.at === 'preview' && (
            <button className="primary" disabled={bytes > step.preview.maxBytes} onClick={() => void send()}>
              Send to {to?.name}
            </button>
          )}
        </div>
      </div>
    </div>,
    host
  );
}
