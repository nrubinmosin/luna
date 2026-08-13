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

const BROWSE = '__browse';

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
  const [pickedFolder, setPickedFolder] = useState<string | null>(null);
  const [model, setModel] = useState<ModelLabel>('Opus');
  const [effort, setEffort] = useState<Effort>('medium');
  const [perm, setPerm] = useState<PermMode>('Bypass');
  const [account, setAccount] = useState(accounts[0]?.name ?? '');
  const [worktree, setWorktree] = useState(true);

  const folderOptions = folders.map(f => ({ value: f.path, label: `${tail2(f.path).parent} / ${tail2(f.path).leaf}` }));
  if (pickedFolder && !folders.some(f => f.path === pickedFolder)) {
    folderOptions.push({ value: pickedFolder, label: `${tail2(pickedFolder).parent} / ${tail2(pickedFolder).leaf}` });
  }

  const effectiveFolder = folder === BROWSE ? '' : folder;
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

  const onFolderChange = async (value: string) => {
    if (value === BROWSE) {
      const picked = await pickFolder();
      if (picked) {
        setPickedFolder(picked);
        setFolder(picked);
      }
      return;
    }
    setFolder(value);
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
    const n = folders.reduce((a, f) => a + f.chats.length, 0) + 1;
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
            <select
              value={folder}
              onChange={e => void onFolderChange(e.target.value)}
              style={{ width: '100%' }}
            >
              {folderOptions.length === 0 && <option value="">— pick a folder —</option>}
              {folderOptions.map(f => (
                <option key={f.value} value={f.value}>{f.label}</option>
              ))}
              <option value={BROWSE}>Browse…</option>
            </select>
            <div style={{ fontSize: 'var(--fs-3)', color: 'var(--faint)', marginTop: 4, whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>
              {effectiveFolder || 'Opens the system folder picker'}
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
