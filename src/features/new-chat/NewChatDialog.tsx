import { useEffect, useState } from 'react';
import type { CSSProperties, ReactNode } from 'react';
import { accountKey, HANDOFF_AT, type ClaudeSettings, type CodexSettings } from '../../shared/types';
import { ACCENT, tail2, tint } from '../../shared/lib/format';
import { useChats } from '../chats/chats.store';
import { findAccount, useAccounts } from '../accounts/accounts.store';
import { claude, codex, ui } from '../providers';
import { startFor, startOf, useNewChat } from './newchat.store';
import { createChat } from './create';
import { useSettingsDraft, type Draft } from './draft';
import { pickFolder } from '../../ipc/cli';
import { chatFolderCheck } from '../../ipc/sessions';
import { codexHooksStart, keepCodexHooks, type CodexHooksStart } from '../../ipc/trust';
import { Icon } from '../../shared/ui/icons';
import { AccountPicker } from './AccountPicker';
import { ThresholdInput } from '../chats/Handoff';

const noteStyle: CSSProperties = { fontSize: 'var(--fs-2)', color: 'var(--faint)', marginTop: 3 };
const linkStyle: CSSProperties = {
  cursor: 'default',
  color: 'var(--dim)',
  textDecoration: 'underline',
  borderRadius: 2
};

/**
 * Where the value on screen came from, or the way back if it was changed.
 * A plain function rather than a component: a component declared inside the
 * dialog is a new type on every render, and React would tear the node down
 * and build it again each time.
 */
function originOf<S extends object>(draft: Draft<S>, describe: (field: keyof S, value: S[keyof S]) => string) {
  return (field: keyof S): ReactNode => {
    if (draft.settings[field] !== draft.resolved[field]) {
      return (
        <div style={noteStyle}>
          just for this chat —{' '}
          <span onClick={() => draft.revert(field)} className="hover-bg" style={linkStyle}>
            back to {describe(field, draft.resolved[field])}
          </span>
        </div>
      );
    }
    const src = draft.from[field];
    return <div style={noteStyle}>{src ? `from ${src}` : 'nothing in settings says — Luna’s own default'}</div>;
  };
}

export function NewChatDialog() {
  const allFolders = useChats(s => s.folders);
  const accounts = useAccounts(s => s.accounts);
  const initialFolder = useNewChat(s => s.initialFolder);
  const onClose = useNewChat(s => s.close);
  // The folders a person launched in, and the one the dialog was opened on
  // even if only an agent has been there.
  const folders = allFolders.filter(f => f.picked || f.path === initialFolder);

  const [folder, setFolder] = useState(
    () => initialFolder ?? useNewChat.getState().lastFolder ?? folders[0]?.path ?? ''
  );
  const [accountId, setAccountId] = useState(() => {
    const last = useNewChat.getState().lastAccount;
    const a = (last && findAccount(accounts, last.provider, last.name)) ?? accounts[0];
    return a ? accountKey(a.provider, a.name) : '';
  });
  const startIn = (path: string) => startFor(useChats.getState().folders.find(f => f.path === path));
  const [worktree, setWorktree] = useState(() => startIn(folder).worktree);
  const [tools, setTools] = useState(() => startIn(folder).tools);
  const [guard, setGuard] = useState(() => startIn(folder).guard);
  // The boxes start the way the folder picked does, so picking another one
  // resets them — a flip made for the first folder was not made for this one.
  const chooseFolder = (path: string) => {
    setFolder(path);
    const start = startIn(path);
    setWorktree(start.worktree);
    setTools(start.tools);
    setGuard(start.guard);
  };
  // Codex's hooks box starts from the Rust side, which keeps it for the
  // background helpers it starts on its own (codex/hooks.rs); asked again for
  // each folder picked, and the box reset to it like the other two.
  const [hooksStart, setHooksStart] = useState<CodexHooksStart>({ default: false, kept: null });
  const [hooks, setHooks] = useState(false);
  useEffect(() => {
    let stale = false;
    void codexHooksStart(folder).then(s => {
      if (stale) return;
      setHooksStart(s);
      setHooks(s.kept ?? s.default);
    });
    return () => {
      stale = true;
    };
  }, [folder]);

  const account = accounts.find(a => accountKey(a.provider, a.name) === accountId) ?? null;
  const provider = account?.provider ?? 'claude';
  const accountPath = account?.path ?? '';
  // Off unless asked for here, and only with Luna tools, whose server offers
  // the handoff (agents/handoff).
  const [autoHandoff, setAutoHandoff] = useState(false);
  const [handoffAt, setHandoffAt] = useState<number>(HANDOFF_AT.default);
  // Codex runs a hook Luna gives it only with hooks' trust bypassed: without
  // them the guard box shows off, whatever the start says, as the chat will be.
  const guardable = provider !== 'codex' || hooks;

  // Read on every render rather than kept: keeping the start below changes it.
  const ownStart = folders.find(f => f.path === folder)?.start;
  const defaultWorktree = useNewChat(s => s.worktree);
  const defaultTools = useNewChat(s => s.tools);
  const defaultGuard = useNewChat(s => s.guard);
  const start = startOf(ownStart, { worktree: defaultWorktree, tools: defaultTools, guard: defaultGuard });
  const startHooks = hooksStart.kept ?? hooksStart.default;
  // A guard box Codex cannot use shows no flip of its own.
  const flipped =
    worktree !== start.worktree ||
    tools !== start.tools ||
    (guardable && guard !== start.guard) ||
    (provider === 'codex' && hooks !== startHooks);
  const keptHere = !!ownStart || (provider === 'codex' && hooksStart.kept !== null);
  // A start that is the defaults anyway is no start of the folder's own. A
  // guard box Codex cannot use keeps what the folder's own start says of it,
  // nothing included: a guard left to Settings stays left to Settings.
  const keepForFolder = () => {
    const keptGuard = guardable ? guard : ownStart?.guard;
    const defaults =
      worktree === defaultWorktree && tools === defaultTools && (keptGuard ?? defaultGuard) === defaultGuard;
    useChats.getState().setStart(folder, defaults ? undefined : { worktree, tools, guard: keptGuard });
    if (provider !== 'codex') return;
    const kept = hooks === hooksStart.default ? null : hooks;
    void keepCodexHooks(folder, kept).then(() => setHooksStart(s => ({ ...s, kept })));
  };

  // One draft per provider, both kept: switching the account between
  // providers and back must not lose what was set on either side.
  const claudeDraft = useSettingsDraft<ClaudeSettings>(
    claude.STOCK,
    claude.settingsDefaults,
    provider === 'claude' ? accountPath : '',
    folder
  );
  const codexDraft = useSettingsDraft<CodexSettings>(
    codex.STOCK,
    codex.settingsDefaults,
    provider === 'codex' ? accountPath : '',
    folder
  );

  // A folder the account has not trusted: whether Luna marks it trusted, as
  // the CLI's own prompt would, is asked here, unticked; left so, the CLI
  // asks in the chat itself. The answer and the tick both belong to one
  // provider, account and folder: picking another makes the answer pending
  // (no Create until it comes) and the tick void, so no trust is ever written
  // for a folder nobody ticked it for.
  const trustKey = `${provider}\u0000${accountPath}\u0000${folder}`;
  const [trustAnswer, setTrustAnswer] = useState<{ key: string; trusted: boolean } | null>(null);
  const [trustTicked, setTrustTicked] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);
  useEffect(() => {
    let stale = false;
    const key = `${provider}\u0000${accountPath}\u0000${folder}`;
    if (!folder || !accountPath) {
      // eslint-disable-next-line react-hooks/set-state-in-effect -- with nothing to ask about the answer is known at once
      setTrustAnswer({ key, trusted: true });
      return;
    }
    void ui(provider)
      .folderTrusted(accountPath, folder)
      .then(trusted => !stale && setTrustAnswer({ key, trusted }))
      .catch(() => !stale && setTrustAnswer({ key, trusted: true }));
    return () => {
      stale = true;
    };
  }, [folder, accountPath, provider]);
  const answer = trustAnswer?.key === trustKey ? trustAnswer : null;
  const trustPending = answer === null;
  const trusted = answer?.trusted ?? true;
  const trustIt = !trusted && trustTicked === trustKey;
  // Claude Code cannot show its trust prompt in a worktree it makes: such a
  // chat needs the folder trusted first, or no worktree.
  const trustNeeded = provider === 'claude' && worktree && !trusted && !trustIt;

  // What the chat would meet in this folder, said here before Create: what
  // keeps it from starting — the folder is not there, or a worktree is asked
  // for outside a git repository — as the start itself would refuse it; and
  // what may still make it fail where Luna cannot tell, which only warns.
  // Asked again for every folder, account and worktree box; no Create until
  // the answer is in.
  const checkKey = `${provider}\u0000${accountPath}\u0000${folder}\u0000${worktree}`;
  const [check, setCheck] = useState<{ key: string; refusal: string | null; warning: string | null } | null>(null);
  useEffect(() => {
    let stale = false;
    const key = `${provider}\u0000${accountPath}\u0000${folder}\u0000${worktree}`;
    if (!folder || !accountPath) {
      // eslint-disable-next-line react-hooks/set-state-in-effect -- with nothing to ask about the answer is known at once
      setCheck({ key, refusal: null, warning: null });
      return;
    }
    void chatFolderCheck(provider, accountPath, folder, worktree)
      .then(c => !stale && setCheck({ key, refusal: c.refusal, warning: c.warning }))
      .catch(e => !stale && setCheck({ key, refusal: String(e), warning: null }));
    return () => {
      stale = true;
    };
  }, [folder, accountPath, provider, worktree]);
  const folderAnswer = check?.key === checkKey ? check : null;
  const refused = folderAnswer?.refusal ?? null;
  const warned = folderAnswer?.warning ?? null;
  const canCreate = !!folder && !!account && !trustPending && !trustNeeded && folderAnswer !== null && !refused;

  const browse = async () => {
    const picked = await pickFolder();
    if (!picked) return;
    // Remembered on the way in, not on create: a folder you went looking for
    // is one you will look for again, even if you close this dialog now.
    useChats.getState().rememberFolder(picked);
    chooseFolder(picked);
  };

  const create = async () => {
    if (!canCreate || creating || !account) return;
    setCreating(true);
    try {
      if (account.provider === 'claude') {
        await createChat({
          provider: 'claude',
          folder,
          account,
          settings: claudeDraft.settings,
          worktree,
          tools,
          guard,
          autoHandoff,
          handoffAt,
          trust: trustIt
        });
      } else {
        await createChat({
          provider: 'codex',
          folder,
          account,
          settings: codexDraft.settings,
          worktree,
          tools,
          hooksUnasked: hooks,
          guard: guard && hooks,
          autoHandoff,
          handoffAt,
          trust: trustIt
        });
      }
    } catch (e) {
      setCreating(false);
      setCreateError(e instanceof Error ? e.message : String(e));
      return;
    }
    setCreating(false);
    onClose();
  };

  // Enter creates: reaching for the mouse to confirm a form that is already
  // filled in is the thing this dialog is trying to save. Escape closes it,
  // through the app keymap.
  //
  // In the capture phase, and swallowed there: the dialog opens over a focused
  // terminal, and xterm cancels the keys it handles — an Enter left to travel
  // would never reach a listener bound the ordinary way, and would post a bare
  // newline into the session behind the dialog on the way past.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Enter' || e.metaKey || e.ctrlKey || e.altKey) return;
      // An open account list takes its own Enter.
      if (document.querySelector('[data-picker-open]')) return;
      e.preventDefault();
      e.stopPropagation();
      // A text box commits on blur; let its own Enter handler run first.
      if (e.target instanceof HTMLInputElement && e.target.type === 'text') {
        e.target.blur();
      }
      void create();
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  });

  const labelStyle: CSSProperties = { fontSize: 'var(--fs-3)', color: 'var(--dim)', marginBottom: 4, fontWeight: 600 };

  return (
    <div
      className="scrim"
      style={{
        position: 'fixed',
        inset: 0,
        background: 'rgba(0,0,0,.32)',
        backdropFilter: 'blur(1px)',
        display: 'grid',
        placeItems: 'center',
        zIndex: 60
      }}
    >
      <div
        className="window"
        style={{ width: 480, boxShadow: 'var(--shadow), var(--border-window-outer), var(--border-window-inner)' }}
      >
        <div className="title-bar">
          <div className="title-bar-text">New chat</div>
          <div className="title-bar-controls">
            <button aria-label="Close" onClick={onClose} />
          </div>
        </div>
        <div className="window-body" style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
          <div style={{ fontSize: 'var(--fs-2)', color: 'var(--faint)' }}>
            {provider === 'codex' ? codex.settingsNote : claude.settingsNote}
          </div>
          <div>
            <div style={labelStyle}>Folder</div>
            {/* A list rather than a select: every folder needs its own way out
                of the list, and a <select> has nowhere to put one. */}
            <div className="xp-field" style={{ maxHeight: 132, overflowY: 'auto', padding: 2 }}>
              {folders.length === 0 && (
                <div style={{ padding: '6px 7px', fontSize: 'var(--fs-3)', color: 'var(--faint)' }}>
                  Nothing here yet — browse for a folder.
                </div>
              )}
              {folders.map(f => {
                const t = tail2(f.path);
                const on = f.path === folder;
                const held = f.chats.length;
                return (
                  <div
                    key={f.id}
                    onClick={() => chooseFolder(f.path)}
                    title={f.path}
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      gap: 6,
                      padding: '3px 6px',
                      cursor: 'default',
                      borderRadius: 4,
                      background: on ? 'var(--row-sel)' : 'transparent',
                      color: on ? 'var(--row-sel-ink)' : 'var(--fg)',
                      fontWeight: on ? 600 : 400,
                      transition: 'background .14s'
                    }}
                    className={on ? undefined : 'hover-bg'}
                  >
                    <span
                      style={{
                        flex: 1,
                        minWidth: 0,
                        overflow: 'hidden',
                        textOverflow: 'ellipsis',
                        whiteSpace: 'nowrap',
                        fontSize: 'var(--fs-4)'
                      }}
                    >
                      <span style={{ opacity: 0.7 }}>{t.parent} / </span>
                      {t.leaf}
                    </span>
                    <span
                      onClick={e => {
                        e.stopPropagation();
                        useChats.getState().removeFolder(f.id);
                        // Forgetting the folder forgets what was kept for it.
                        if (!held) void keepCodexHooks(f.path, null);
                        if (on) setFolder('');
                      }}
                      title={held ? 'Still holds chats — delete those first' : 'Forget this folder'}
                      className={held ? undefined : 'hover-danger'}
                      style={{
                        width: 18,
                        height: 18,
                        flex: 'none',
                        display: 'grid',
                        placeItems: 'center',
                        borderRadius: 4,
                        cursor: 'default',
                        opacity: held ? 0.3 : 0.8
                      }}
                    >
                      <Icon name="close" size={13} />
                    </span>
                  </div>
                );
              })}
            </div>
            <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginTop: 5 }}>
              <button className="slim" onClick={() => void browse()}>
                Browse…
              </button>
              <span
                style={{
                  flex: 1,
                  minWidth: 0,
                  fontSize: 'var(--fs-2)',
                  color: 'var(--faint)',
                  whiteSpace: 'nowrap',
                  overflow: 'hidden',
                  textOverflow: 'ellipsis'
                }}
              >
                {folder || 'no folder chosen'}
              </span>
            </div>
            {!trusted && folder && account && (
              <div
                style={{
                  marginTop: 7,
                  padding: '7px 9px',
                  borderRadius: 2,
                  fontSize: 'var(--fs-3)',
                  lineHeight: 1.45,
                  border: `1px solid ${ACCENT}`,
                  background: tint(10, 'transparent'),
                  color: 'var(--dim)'
                }}
              >
                <div className="field-row" style={{ marginBottom: 3 }}>
                  <input
                    type="checkbox"
                    id="trust-toggle"
                    checked={trustIt}
                    onChange={() => setTrustTicked(trustIt ? null : trustKey)}
                  />
                  <label htmlFor="trust-toggle" style={{ fontWeight: 600, color: 'var(--fg)', cursor: 'default' }}>
                    Trust this folder in “{account.name}”
                  </label>
                </div>
                {ui(provider).trustNote}
                {trustNeeded && (
                  <div style={{ marginTop: 4, color: 'var(--fg)' }}>
                    Claude Code cannot ask about trust in a worktree it makes: tick this, or turn Git worktree off.
                  </div>
                )}
              </div>
            )}
            {refused && (
              <div role="alert" style={{ marginTop: 7, fontSize: 'var(--fs-3)', color: 'oklch(.58 .2 25)' }}>
                {refused}
              </div>
            )}
            {warned && (
              <div role="status" style={{ marginTop: 7, fontSize: 'var(--fs-3)', color: 'var(--dim)' }}>
                {warned}
              </div>
            )}
            {createError && (
              <div style={{ marginTop: 7, fontSize: 'var(--fs-3)', color: 'oklch(.58 .2 25)' }}>
                Could not create the chat: {createError}
              </div>
            )}
          </div>

          <div style={{ display: 'flex', gap: 12 }}>
            <div style={{ flex: 1, minWidth: 0 }}>
              <div style={labelStyle}>Account</div>
              <AccountPicker accounts={accounts} value={accountId} onChange={setAccountId} />
            </div>
            <div style={{ width: 150, flex: 'none' }}>
              <div style={labelStyle}>Isolation</div>
              <div
                className="field-row"
                title={
                  provider === 'codex'
                    ? 'Codex makes a git worktree of its own (detached HEAD, under the account folder) and runs inside it'
                    : 'Run the session in an isolated git worktree'
                }
                style={{ height: 'calc(var(--ui) * 1.6)' }}
              >
                <input type="checkbox" id="worktree-toggle" checked={worktree} onChange={() => setWorktree(w => !w)} />
                <label htmlFor="worktree-toggle" style={{ whiteSpace: 'nowrap', cursor: 'default' }}>
                  Git worktree
                </label>
              </div>
              <div
                className="field-row"
                title={
                  "Attach Luna's MCP tools: the session can spawn helper sessions (another model or " +
                  'account), send them text, read their replies, wait for them, kill and delete them. ' +
                  'Costs about 2k tokens of context once; a plain chat knows nothing of Luna.'
                }
                style={{ height: 'calc(var(--ui) * 1.6)' }}
              >
                <input type="checkbox" id="tools-toggle" checked={tools} onChange={() => setTools(t => !t)} />
                <label htmlFor="tools-toggle" style={{ whiteSpace: 'nowrap', cursor: 'default' }}>
                  Luna tools
                </label>
              </div>
              {provider === 'codex' && (
                <div
                  className="field-row"
                  title={
                    'Run the project’s hooks without Codex asking to trust them first ' +
                    '(--dangerously-bypass-hook-trust). Codex asks again for every new worktree, account ' +
                    'or edited hook, and a background helper cannot ask at all; its helpers take this ' +
                    'folder’s start.'
                  }
                  style={{ height: 'calc(var(--ui) * 1.6)' }}
                >
                  <input type="checkbox" id="hooks-toggle" checked={hooks} onChange={() => setHooks(h => !h)} />
                  <label htmlFor="hooks-toggle" style={{ whiteSpace: 'nowrap', cursor: 'default' }}>
                    Hooks unasked
                  </label>
                </div>
              )}
              <div
                className="field-row"
                title={
                  guardable
                    ? 'Guard against endless waits: a shell command that would wait forever (a server, a watcher, ' +
                      'a prompt for input) is refused before it runs, with the reason for the model. Off, the CLI ' +
                      'runs every command as it would on its own.'
                    : 'Codex runs a hook Luna gives it only with Hooks unasked on'
                }
                style={{ height: 'calc(var(--ui) * 1.6)' }}
              >
                <input
                  type="checkbox"
                  id="guard-toggle"
                  checked={guard && guardable}
                  disabled={!guardable}
                  onChange={() => setGuard(g => !g)}
                />
                <label htmlFor="guard-toggle" style={{ whiteSpace: 'nowrap', cursor: 'default' }}>
                  Wait guard
                </label>
              </div>
            </div>
          </div>
          {folder && (
            <div style={{ ...noteStyle, marginTop: -8, textAlign: 'right' }}>
              {flipped ? (
                <>
                  just for this chat —{' '}
                  <span
                    onClick={() => {
                      setWorktree(start.worktree);
                      setTools(start.tools);
                      setGuard(start.guard);
                      setHooks(startHooks);
                    }}
                    className="hover-bg"
                    style={linkStyle}
                  >
                    back to {keptHere ? 'this folder’s start' : 'the default'}
                  </span>
                  {' · '}
                  <span onClick={keepForFolder} className="hover-bg" style={linkStyle}>
                    keep for this folder
                  </span>
                </>
              ) : keptHere ? (
                'as kept for this folder'
              ) : (
                'as set in Settings'
              )}
            </div>
          )}

          <div
            className="field-row"
            title={
              tools
                ? 'Past this share of its context, Luna asks the chat to write a handoff; on its word the chat goes on ' +
                  'in a fresh conversation from it, its helpers and their answers carrying on. Nothing switches ' +
                  'unless the chat asks.'
                : 'Only with Luna tools: their server is what offers the handoff'
            }
          >
            <input
              type="checkbox"
              id="handoff-toggle"
              checked={autoHandoff && tools}
              disabled={!tools}
              onChange={() => setAutoHandoff(h => !h)}
            />
            <label htmlFor="handoff-toggle" style={{ whiteSpace: 'nowrap', cursor: 'default' }}>
              Auto-handoff at
            </label>
            <ThresholdInput
              id="handoff-at"
              value={handoffAt}
              disabled={!tools || !autoHandoff}
              onCommit={setHandoffAt}
            />
            <span style={{ fontSize: 'var(--fs-3)', color: 'var(--dim)', whiteSpace: 'nowrap' }}>% of its context</span>
          </div>

          {provider === 'codex' ? (
            <codex.Fields draft={codexDraft} origin={originOf(codexDraft, codex.describe)} accountPath={accountPath} />
          ) : (
            <claude.Fields
              draft={claudeDraft}
              origin={originOf(claudeDraft, claude.describe)}
              accountPath={accountPath}
            />
          )}
        </div>
        {/* Out of the body, which scrolls on a short screen: Create stays in reach. */}
        <div className="dialog-foot">
          <button onClick={onClose}>Cancel</button>
          <button onClick={() => void create()} disabled={!canCreate || creating} className="primary">
            {creating ? 'Creating…' : 'Create'}
          </button>
        </div>
      </div>
    </div>
  );
}
