/**
 * "Move to another account…": the chat goes on as a new chat on another
 * account of its CLI, from a document the person reads, may edit, and sends
 * (docs/adr/0023-moving-a-chat-to-another-account.md). Two steps: what goes
 * where (the account, who writes the document, the old chat kept or stopped,
 * a note), then the whole of what goes, the line Luna types and the
 * document, with Send bound to exactly that text.
 */
import { useEffect, useId, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import {
  moveAsk,
  moveCancel,
  moveConfirm,
  moveList,
  movePrepare,
  movePreview,
  sha256Hex,
  type MovePreviewDto
} from '../../ipc/moving';
import { accountKey, PROVIDER_LABEL, type Account, type Chat } from '../../shared/types';
import { useModal } from '../../shared/ui/modal';
import { useAccounts } from '../accounts/accounts.store';
import { AccountPicker } from '../new-chat/AccountPicker';
import { ui } from '../providers';
import { useChats, newId } from './chats.store';
import { carryOut, launchFor, modelFor } from './moving';
import { useMoveHints } from './moveHints.store';

/** How often the dialog looks whether the chat has written its document. */
const ASKED_EVERY_MS = 2000;

/** The first account that is not this chat's, of its CLI, with every window
 *  under 95%; else the first of them. */
export function defaultTarget(accounts: Account[], chat: Chat): Account | null {
  const others = accounts.filter(a => a.provider === chat.provider && a.name !== chat.account);
  return others.find(a => a.haveUsage && a.limits.every(l => l.used < 0.95)) ?? others[0] ?? null;
}

/** Why "Ask the chat" is not offered, or null where it is. */
export function askRefusal(chat: Chat, limit: string | null): string | null {
  if (!chat.tools || !chat.autoHandoff) return 'Only a chat with auto-handoff on writes its own document.';
  if (chat.provider === 'claude' && chat.perm === 'Plan only') return 'In plan mode the chat writes no file.';
  if (chat.provider === 'codex' && (chat.sandbox === 'read-only' || chat.sandbox === 'read-only-net'))
    return "Codex's read-only sandbox writes no file.";
  // A chat at rest may well be running: whether its CLI runs is the core's to
  // say, which refuses the ask of one that does not.
  if (limit) return 'The chat stopped on a usage limit and cannot write now.';
  return null;
}

type Step =
  | { at: 'setup' }
  | { at: 'asking'; moveId: string; newId: string }
  | { at: 'preview'; preview: MovePreviewDto; text: string }
  | { at: 'sending' };

export function MoveChatDialog({ chat, onClose }: { chat: Chat; onClose: () => void }) {
  const accounts = useAccounts(s => s.accounts);
  const folder = useChats(s => s.folderOf(chat.id));
  const folders = useChats(s => s.folders);
  const usedAccounts = useMemo(
    () => new Set(folders.flatMap(f => f.chats.map(c => accountKey(c.provider, c.account)))),
    [folders]
  );
  const limit = useMoveHints(s => s.limits[chat.id] ?? null);
  const targets = useMemo(
    () => accounts.filter(a => a.provider === chat.provider && a.name !== chat.account),
    [accounts, chat.provider, chat.account]
  );
  const [target, setTarget] = useState(() => {
    const t = defaultTarget(accounts, chat);
    return t ? accountKey(t.provider, t.name) : '';
  });
  const to = targets.find(a => accountKey(a.provider, a.name) === target) ?? null;
  const codexWorktree = chat.provider === 'codex' && !!chat.worktreePath;
  const claudeWorktree = chat.provider === 'claude' && chat.worktree;
  const refusal = askRefusal(chat, limit);
  const [source, setSource] = useState<'luna' | 'chat'>('luna');
  const [stopOld, setStopOld] = useState(claudeWorktree);
  const [note, setNote] = useState('');
  const [step, setStep] = useState<Step>({ at: 'setup' });
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

  // A chat asked for its document: looked at until it is in, or the ask ends.
  useEffect(() => {
    if (step.at !== 'asking') return;
    let stopped = false;
    const look = async () => {
      const m = (await moveList().catch(() => [])).find(x => x.moveId === step.moveId);
      if (stopped) return;
      if (m?.phase === 'shown') {
        const preview = await movePreview(chat.id, step.moveId).catch((e: unknown) => {
          setError(String(e));
          return null;
        });
        if (preview && !stopped) setStep({ at: 'preview', preview, text: preview.text });
        return;
      }
      if (m && (m.phase === 'failed' || m.phase === 'canceled')) {
        setError(m.why ?? 'The chat did not write its document.');
        setStep({ at: 'setup' });
        setSource('luna');
        return;
      }
      timer = setTimeout(() => void look(), ASKED_EVERY_MS);
    };
    let timer = setTimeout(() => void look(), ASKED_EVERY_MS);
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  }, [step, chat.id]);

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
  const canGo = !!to && !!folder && !!model && trusted !== null && !trustNeeded && !codexWorktree && !busy;

  const build = async () => {
    if (!to || !folder) return;
    setBusy(true);
    setError(null);
    const id = newId('c');
    try {
      if (source === 'chat') {
        const s = await moveAsk(chat.id, id, to.path, note);
        if (began(s.moveId)) setStep({ at: 'asking', moveId: s.moveId, newId: id });
      } else {
        const preview = await movePrepare(chat.id, id, to.path, chat.sessionId ?? null, note);
        if (began(preview.moveId)) setStep({ at: 'preview', preview, text: preview.text });
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const send = async () => {
    if (step.at !== 'preview' || !to || !folder || !model) return;
    const { preview, text } = step;
    setStep({ at: 'sending' });
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
      setStep({ at: 'preview', preview, text });
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
  const trap = useModal(box, host);
  const titleId = useId();
  const label = PROVIDER_LABEL[chat.provider];
  const bytes = step.at === 'preview' ? new TextEncoder().encode(step.text).length : 0;

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
                <label className="move-check">
                  <input type="checkbox" checked={trustIt} onChange={e => setTrustIt(e.target.checked)} />
                  Trust {folder?.path} on {to.name}
                  {claudeWorktree ? ' (needed: in a worktree Claude Code cannot ask)' : ''}
                </label>
              )}
              <fieldset className="move-group">
                <legend>The document</legend>
                <label className="move-check">
                  <input
                    type="radio"
                    name="move-source"
                    checked={source === 'luna'}
                    onChange={() => setSource('luna')}
                  />
                  Luna writes it from the chat's records
                </label>
                <label className="move-check" title={refusal ?? undefined}>
                  <input
                    type="radio"
                    name="move-source"
                    disabled={!!refusal}
                    checked={source === 'chat'}
                    onChange={() => setSource('chat')}
                  />
                  Ask the chat to write it{refusal ? ` — ${refusal}` : ''}
                </label>
              </fieldset>
              <fieldset className="move-group">
                <legend>This chat</legend>
                <label className="move-check">
                  <input
                    type="radio"
                    name="move-old"
                    disabled={claudeWorktree}
                    checked={!stopOld}
                    onChange={() => setStopOld(false)}
                  />
                  Keep it
                </label>
                <label className="move-check">
                  <input type="radio" name="move-old" checked={stopOld} onChange={() => setStopOld(true)} />
                  Stop it
                  {claudeWorktree ? ' (always, in a Claude Code worktree: the new chat goes into it)' : ''}
                </label>
                {!stopOld && <div className="move-warn">Kept, both chats work in one folder.</div>}
                {stopOld && chat.status === 'background' && (
                  <div className="move-warn">Its subagents and jobs still at work stop with it.</div>
                )}
              </fieldset>
              <label className="move-row move-note">
                <span>Your note</span>
                <textarea
                  rows={2}
                  value={note}
                  placeholder="What the new chat should do first (optional)"
                  onChange={e => setNote(e.target.value)}
                />
              </label>
            </>
          ) : step.at === 'asking' ? (
            <p>
              The chat is asked to write its document, once it is at its prompt. Nothing is cleared there; if Claude
              Code asks to create the file, answer in its pane.
            </p>
          ) : step.at === 'preview' ? (
            <>
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
              disabled={!canGo || (source === 'chat' && !!refusal)}
              onClick={() => void build()}
            >
              {source === 'chat' ? 'Ask the chat' : 'Preview'}
            </button>
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
