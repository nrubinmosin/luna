import { useEffect, useState } from 'react';
import type { CSSProperties } from 'react';
import { EFFORTS, MODELS, PERM_HINTS } from '../../shared/types';
import type { Effort, ModelLabel, PermMode } from '../../shared/types';
import { ACCENT, tail2, tint } from '../../shared/lib/format';
import { newId, useChats } from '../chats/chats.store';
import { usePanes } from '../panes/panes.store';
import { useAccounts } from '../accounts/accounts.store';
import { useNewChat } from './newchat.store';
import { folderTrusted, pickFolder, trustFolder } from '../../ipc/commands';

const segWrap: CSSProperties = { display: 'flex', gap: 2, padding: 2, background: 'var(--chip)', borderRadius: 2 };

function Segmented<T extends string>({ items, value, onPick, height = 24 }: {
  items: readonly T[];
  value: T;
  onPick: (v: T) => void;
  height?: number;
}) {
  return (
    <div className="xp-sunken" style={segWrap}>
      {items.map(item => (
        <div
          key={item}
          onClick={() => onPick(item)}
          className={value !== item ? 'hover-bg' : undefined}
          style={{
            flex: 1, height, borderRadius: 2, display: 'grid', placeItems: 'center', fontSize: 'var(--fs-4)',
            fontWeight: 600, cursor: 'default', whiteSpace: 'nowrap',
            background: value === item ? 'var(--bg)' : 'transparent',
            color: value === item ? 'var(--fg)' : 'var(--dim)',
            boxShadow: value === item ? 'var(--border-sunken-outer), var(--border-sunken-inner)' : 'none'
          }}
        >
          {item}
        </div>
      ))}
    </div>
  );
}

export function NewChatDialog() {
  const folders = useChats(s => s.folders);
  const accounts = useAccounts(s => s.accounts);
  const initialFolder = useNewChat(s => s.initialFolder);
  const onClose = useNewChat(s => s.close);
  const { addChat } = useChats.getState();
  const { autoPlace } = usePanes.getState();

  const [folder, setFolder] = useState(initialFolder ?? folders[0]?.path ?? '');
  const group = usePanes(s => s.group);
  const [model, setModel] = useState<ModelLabel>('Opus');
  const [effort, setEffort] = useState<Effort>('medium');
  const [perm, setPerm] = useState<PermMode>('Bypass');
  const [account, setAccount] = useState(accounts[0]?.name ?? '');
  const [worktree, setWorktree] = useState(true);

  const effectiveFolder = folder;
  const canCreate = !!effectiveFolder && !!account;
  const accountPath = accounts.find(a => a.name === account)?.path ?? '';

  // The CLI can't show its trust prompt under --worktree; it just tells you to
  // open the folder without isolation first. Detect the untrusted case up front
  // so creating the chat can write the same bit the prompt would.
  const [trusted, setTrusted] = useState(true);
  const [creating, setCreating] = useState(false);
  const [trustError, setTrustError] = useState<string | null>(null);
  useEffect(() => {
    let stale = false;
    if (!effectiveFolder || !accountPath) {
      setTrusted(true);
      return;
    }
    void folderTrusted(accountPath, effectiveFolder)
      .then(ok => !stale && setTrusted(ok))
      .catch(() => !stale && setTrusted(true));
    return () => {
      stale = true;
    };
  }, [effectiveFolder, accountPath]);

  const browse = async () => {
    const picked = await pickFolder();
    if (!picked) return;
    // Remembered on the way in, not on create: a folder you went looking for
    // is one you will look for again, even if you close this dialog now.
    useChats.getState().rememberFolder(picked);
    setFolder(picked);
  };

  const create = async () => {
    if (!canCreate || creating) return;
    // Must land *before* the session spawns — adding the chat mounts the
    // terminal, which starts the CLI immediately.
    if (!trusted) {
      setCreating(true);
      try {
        await trustFolder(accountPath, effectiveFolder);
      } catch (e) {
        setCreating(false);
        setTrustError(String(e));
        return;
      }
      setCreating(false);
    }
    const n = folders.reduce((a, f) => a + f.chats.filter(c => c.group === group).length, 0) + 1;
    const id = newId('c');
    addChat(effectiveFolder, {
      id,
      name: `chat ${n}`,
      status: 'resting',
      model,
      effort,
      perm,
      context: 0,
      account,
      group,
      worktree
    });
    autoPlace(id);
    onClose();
  };

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
          <div style={{ fontSize: 'var(--fs-2)', color: 'var(--faint)' }}>Model, effort and permission mode can be changed later</div>
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
                    {held > 0 && (
                      <span style={{ fontSize: 'var(--fs-1)', opacity: 0.7 }}>{held}</span>
                    )}
                    <span
                      onClick={e => {
                        e.stopPropagation();
                        useChats.getState().removeFolder(f.id);
                        if (on) setFolder('');
                      }}
                      title={
                        held
                          ? `Still holds ${held} chat${held > 1 ? 's' : ''} — delete those first`
                          : 'Forget this folder'
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
                {effectiveFolder || 'no folder chosen'}
              </span>
            </div>
            {!trusted && effectiveFolder && (
              <div
                style={{
                  marginTop: 7, padding: '7px 9px', borderRadius: 2, fontSize: 'var(--fs-3)', lineHeight: 1.45,
                  border: `1px solid ${ACCENT}`, background: tint(10, 'transparent'), color: 'var(--dim)'
                }}
              >
                <span style={{ fontWeight: 600, color: 'var(--fg)' }}>New folder for “{account}”.</span>{' '}
                Creating the chat marks it trusted for this account — the same thing Claude Code's
                trust prompt does. The prompt can't be shown under an isolated worktree, which is why
                it otherwise asks you to open the folder without isolation first.
              </div>
            )}
            {trustError && (
              <div style={{ marginTop: 7, fontSize: 'var(--fs-3)', color: 'oklch(.58 .2 25)' }}>
                Could not write trust setting: {trustError}
              </div>
            )}
          </div>

          <div>
            <div style={labelStyle}>Model</div>
            <Segmented items={MODELS} value={model} onPick={setModel} />
          </div>

          <div>
            <div style={labelStyle}>Effort</div>
            <Segmented items={EFFORTS} value={effort} onPick={setEffort} />
          </div>

          <div style={{ display: 'flex', gap: 12 }}>
            <div style={{ flex: 1, minWidth: 0 }}>
              <div style={labelStyle}>Account</div>
              <select
                value={account}
                onChange={e => setAccount(e.target.value)}
                style={{ width: '100%' }}
              >
                {accounts.length === 0 && <option value="">— add an account first —</option>}
                {accounts.map(a => (
                  <option key={a.name} value={a.name}>{a.name}</option>
                ))}
              </select>
            </div>
            <div style={{ width: 150, flex: 'none' }}>
              <div style={labelStyle}>Isolation</div>
              <div
                className="field-row"
                title="Run the session in an isolated git worktree"
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
            </div>
          </div>

          <div>
            <div style={labelStyle}>Permission mode</div>
            <div style={{ display: 'flex', flexDirection: 'column', gap: 5 }}>
              {(Object.keys(PERM_HINTS) as PermMode[]).map(p => {
                const on = perm === p;
                const id = `perm-${p}`;
                return (
                  <div key={p} className="field-row">
                    <input
                      type="radio"
                      id={id}
                      name="perm-mode"
                      checked={on}
                      onChange={() => setPerm(p)}
                    />
                    <label htmlFor={id} style={{ fontWeight: 600, whiteSpace: 'nowrap', cursor: 'default' }}>
                      {p}
                    </label>
                    <span style={{ fontSize: 'var(--fs-2)', color: 'var(--faint)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{PERM_HINTS[p]}</span>
                  </div>
                );
              })}
            </div>
          </div>

          <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8, marginTop: 2 }}>
            <button onClick={onClose}>Cancel</button>
            <button onClick={() => void create()} disabled={!canCreate || creating} className="primary">
              {creating ? 'Trusting…' : 'Create'}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
