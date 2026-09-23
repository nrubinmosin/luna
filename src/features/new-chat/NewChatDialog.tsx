import { useEffect, useState } from 'react';
import type { CSSProperties, ReactNode } from 'react';
import { accountKey, PROVIDER_LABEL, type ClaudeSettings, type CodexSettings } from '../../shared/types';
import { ACCENT, tail2, tint } from '../../shared/lib/format';
import { useChats } from '../chats/chats.store';
import { findAccount, useAccounts } from '../accounts/accounts.store';
import { claude, codex, ui } from '../providers';
import { useNewChat } from './newchat.store';
import { createChat } from './create';
import { useSettingsDraft, type Draft } from './draft';
import { pickFolder } from '../../ipc/commands';

const noteStyle: CSSProperties = { fontSize: 'var(--fs-2)', color: 'var(--faint)', marginTop: 3 };

/**
 * Where the value on screen came from, or the way back if it was changed.
 * A plain function rather than a component: a component declared inside the
 * dialog is a new type on every render, and React would tear the node down
 * and build it again each time.
 */
function originOf<S extends object>(
  draft: Draft<S>,
  describe: (field: keyof S, value: S[keyof S]) => string
) {
  return (field: keyof S): ReactNode => {
    if (draft.settings[field] !== draft.resolved[field]) {
      return (
        <div style={noteStyle}>
          just for this chat —{' '}
          <span
            onClick={() => draft.revert(field)}
            className="hover-bg"
            style={{ cursor: 'default', color: 'var(--dim)', textDecoration: 'underline', borderRadius: 2 }}
          >
            back to {describe(field, draft.resolved[field])}
          </span>
        </div>
      );
    }
    const src = draft.from[field];
    return (
      <div style={noteStyle}>
        {src ? `from ${src}` : 'nothing in settings says — Luna’s own default'}
      </div>
    );
  };
}

export function NewChatDialog() {
  const folders = useChats(s => s.folders);
  const accounts = useAccounts(s => s.accounts);
  const initialFolder = useNewChat(s => s.initialFolder);
  const onClose = useNewChat(s => s.close);

  const [folder, setFolder] = useState(
    () => initialFolder ?? useNewChat.getState().lastFolder ?? folders[0]?.path ?? ''
  );
  const [accountId, setAccountId] = useState(() => {
    const last = useNewChat.getState().lastAccount;
    const a = (last && findAccount(accounts, last.provider, last.name)) ?? accounts[0];
    return a ? accountKey(a.provider, a.name) : '';
  });
  const [worktree, setWorktree] = useState(() => useNewChat.getState().lastWorktree);
  // Off every time: a session with tools pays ~2k tokens of context for
  // them, and a plain chat should not carry that by habit.
  const [tools, setTools] = useState(false);

  const account = accounts.find(a => accountKey(a.provider, a.name) === accountId) ?? null;
  const provider = account?.provider ?? 'claude';
  const accountPath = account?.path ?? '';
  const canCreate = !!folder && !!account;

  // One draft per provider, both kept: switching the account between
  // providers and back must not lose what was set on either side.
  const claudeDraft = useSettingsDraft<ClaudeSettings>(claude.STOCK, claude.settingsDefaults, provider === 'claude' ? accountPath : '', folder);
  const codexDraft = useSettingsDraft<CodexSettings>(codex.STOCK, codex.settingsDefaults, provider === 'codex' ? accountPath : '', folder);

  // Neither CLI can show its trust prompt the way Luna runs it; detect the
  // untrusted case up front so creating the chat can write the bit itself.
  const [trusted, setTrusted] = useState(true);
  const [creating, setCreating] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);
  useEffect(() => {
    let stale = false;
    if (!folder || !accountPath) {
      setTrusted(true);
      return;
    }
    void ui(provider).folderTrusted(accountPath, folder)
      .then(ok => !stale && setTrusted(ok))
      .catch(() => !stale && setTrusted(true));
    return () => {
      stale = true;
    };
  }, [folder, accountPath, provider]);

  const browse = async () => {
    const picked = await pickFolder();
    if (!picked) return;
    // Remembered on the way in, not on create: a folder you went looking for
    // is one you will look for again, even if you close this dialog now.
    useChats.getState().rememberFolder(picked);
    setFolder(picked);
  };

  const create = async () => {
    if (!canCreate || creating || !account) return;
    setCreating(true);
    try {
      if (account.provider === 'claude') {
        await createChat({ provider: 'claude', folder, account, settings: claudeDraft.settings, worktree, tools });
      } else {
        await createChat({ provider: 'codex', folder, account, settings: codexDraft.settings, worktree, tools });
      }
    } catch (e) {
      setCreating(false);
      setCreateError(String(e));
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
    <div style={{ position: 'fixed', inset: 0, background: 'rgba(0,0,0,.32)', backdropFilter: 'blur(1px)', display: 'grid', placeItems: 'center', zIndex: 60 }}>
      <div className="window" style={{ width: 480, boxShadow: 'var(--shadow), var(--border-window-outer), var(--border-window-inner)' }}>
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
            <div
              className="xp-field"
              style={{ background: '#fff', maxHeight: 132, overflowY: 'auto', padding: 2 }}
            >
              {folders.length === 0 && (
                <div style={{ padding: '6px 7px', fontSize: 'var(--fs-3)', color: '#666' }}>
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
                    onClick={() => setFolder(f.path)}
                    title={f.path}
                    style={{
                      display: 'flex', alignItems: 'center', gap: 6, padding: '3px 6px', cursor: 'default',
                      background: on ? 'var(--dialog-blue)' : 'transparent',
                      color: on ? '#fff' : '#000'
                    }}
                  >
                    <span style={{ flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', fontSize: 'var(--fs-4)' }}>
                      <span style={{ opacity: 0.7 }}>{t.parent} / </span>
                      {t.leaf}
                    </span>
                    <span
                      onClick={e => {
                        e.stopPropagation();
                        useChats.getState().removeFolder(f.id);
                        if (on) setFolder('');
                      }}
                      title={
                        held ? 'Still holds chats — delete those first' : 'Forget this folder'
                      }
                      className={held ? undefined : 'hover-danger'}
                      style={{
                        width: 15, height: 15, flex: 'none', display: 'grid', placeItems: 'center',
                        fontSize: 'var(--fs-1)', cursor: 'default', opacity: held ? 0.25 : 0.65
                      }}
                    >
                      ✕
                    </span>
                  </div>
                );
              })}
            </div>
            <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginTop: 5 }}>
              <button className="slim" onClick={() => void browse()}>Browse…</button>
              <span style={{ flex: 1, minWidth: 0, fontSize: 'var(--fs-2)', color: 'var(--faint)', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>
                {folder || 'no folder chosen'}
              </span>
            </div>
            {!trusted && folder && account && (
              <div
                style={{
                  marginTop: 7, padding: '7px 9px', borderRadius: 2, fontSize: 'var(--fs-3)', lineHeight: 1.45,
                  border: `1px solid ${ACCENT}`, background: tint(10, 'transparent'), color: 'var(--dim)'
                }}
              >
                <span style={{ fontWeight: 600, color: 'var(--fg)' }}>New folder for “{account.name}”.</span>{' '}
                {ui(provider).trustNote}
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
              <select
                value={accountId}
                onChange={e => setAccountId(e.target.value)}
                style={{ width: '100%' }}
              >
                {accounts.length === 0 && <option value="">— add an account first —</option>}
                {accounts.map(a => (
                  <option key={accountKey(a.provider, a.name)} value={accountKey(a.provider, a.name)}>
                    {a.name} · {PROVIDER_LABEL[a.provider]}
                  </option>
                ))}
              </select>
            </div>
            <div style={{ width: 150, flex: 'none' }}>
              <div style={labelStyle}>Isolation</div>
              <div
                className="field-row"
                title={
                  provider === 'codex'
                    ? 'Luna makes a git worktree under .codex/worktrees and runs Codex inside it'
                    : 'Run the session in an isolated git worktree'
                }
                style={{ height: 'calc(var(--ui) * 1.6)' }}
              >
                <input
                  type="checkbox"
                  id="worktree-toggle"
                  checked={worktree}
                  onChange={() => setWorktree(w => !w)}
                />
                <label htmlFor="worktree-toggle" style={{ whiteSpace: 'nowrap', cursor: 'default' }}>
                  Git worktree
                </label>
              </div>
              <div
                className="field-row"
                title={
                  'Attach Luna\'s MCP tools: the session can spawn helper sessions (another model or ' +
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
            </div>
          </div>

          {provider === 'codex' ? (
            <codex.Fields draft={codexDraft} origin={originOf(codexDraft, codex.describe)} accountPath={accountPath} />
          ) : (
            <claude.Fields draft={claudeDraft} origin={originOf(claudeDraft, claude.describe)} />
          )}

          <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8, marginTop: 2 }}>
            <button onClick={onClose}>Cancel</button>
            <button onClick={() => void create()} disabled={!canCreate || creating} className="primary">
              {creating ? 'Creating…' : 'Create'}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
