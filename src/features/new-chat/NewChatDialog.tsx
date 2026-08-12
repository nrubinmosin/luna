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

const segWrap: CSSProperties = { display: 'flex', gap: 3, padding: 3, background: 'var(--chip)', borderRadius: 9 };

function Segmented<T extends string>({ items, value, onPick, height = 28 }: {
  items: readonly T[];
  value: T;
  onPick: (v: T) => void;
  height?: number;
}) {
  return (
    <div style={segWrap}>
      {items.map(item => (
        <div
          key={item}
          onClick={() => onPick(item)}
          style={{
            flex: 1, height, borderRadius: 6, display: 'grid', placeItems: 'center', fontSize: 13,
            fontWeight: 500, cursor: 'default', whiteSpace: 'nowrap',
            background: value === item ? 'var(--bg)' : 'transparent',
            color: value === item ? 'var(--fg)' : 'var(--dim)',
            boxShadow: value === item ? '0 1px 2px oklch(.3 .04 160 / .18)' : 'none'
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
  const [model, setModel] = useState<ModelLabel>('Sonnet');
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

  const labelStyle: CSSProperties = { fontSize: 12, color: 'var(--dim)', marginBottom: 5 };

  return (
    <div style={{ position: 'fixed', inset: 0, background: 'oklch(.2 .03 160 / .3)', backdropFilter: 'blur(2px)', display: 'grid', placeItems: 'center', zIndex: 60 }}>
      <div style={{ width: 500, background: 'var(--bg)', border: '1px solid var(--line)', borderRadius: 14, boxShadow: 'var(--shadow)', overflow: 'hidden' }}>
        <div style={{ padding: '16px 18px 12px', borderBottom: '1px solid var(--line)', background: 'var(--panel)' }}>
          <div style={{ fontSize: 16.5, fontWeight: 640 }}>New chat</div>
          <div style={{ fontSize: 12.5, color: 'var(--faint)', marginTop: 2 }}>Model, effort and permission mode can be changed later</div>
        </div>

        <div style={{ padding: '16px 18px', display: 'flex', flexDirection: 'column', gap: 14 }}>
          <div>
            <div style={labelStyle}>Folder</div>
            <select
              value={folder}
              onChange={e => void onFolderChange(e.target.value)}
              style={{ width: '100%', height: 34, padding: '0 8px', border: '1px solid var(--line)', borderRadius: 8, background: 'var(--panel)', color: 'var(--fg)', font: 'inherit', fontSize: 13.5, outline: 'none' }}
            >
              {folderOptions.length === 0 && <option value="">— pick a folder —</option>}
              {folderOptions.map(f => (
                <option key={f.value} value={f.value}>{f.label}</option>
              ))}
              <option value={BROWSE}>Browse…</option>
            </select>
            <div style={{ fontSize: 11.5, color: 'var(--faint)', marginTop: 4, whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>
              {effectiveFolder || 'Opens the system folder picker'}
            </div>
            {!trusted && effectiveFolder && (
              <div
                style={{
                  marginTop: 7, padding: '7px 9px', borderRadius: 8, fontSize: 11.5, lineHeight: 1.45,
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
              <div style={{ marginTop: 7, fontSize: 11.5, color: 'oklch(.58 .2 25)' }}>
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
                style={{ width: '100%', height: 34, padding: '0 6px', border: '1px solid var(--line)', borderRadius: 8, background: 'var(--panel)', color: 'var(--fg)', font: 'inherit', fontSize: 13.5, outline: 'none' }}
              >
                {accounts.length === 0 && <option value="">— add an account first —</option>}
                {accounts.map(a => (
                  <option key={a.name} value={a.name}>{a.name}</option>
                ))}
              </select>
            </div>
            <div style={{ width: 170, flex: 'none' }}>
              <div style={labelStyle}>Isolation</div>
              <div
                onClick={() => setWorktree(w => !w)}
                title="Run the session in an isolated git worktree"
                style={{
                  display: 'flex', alignItems: 'center', gap: 8, height: 34, padding: '0 9px',
                  border: `1px solid ${worktree ? ACCENT : 'var(--line)'}`, borderRadius: 8, cursor: 'default',
                  background: worktree ? tint(11, 'transparent') : 'transparent'
                }}
              >
                <span
                  style={{
                    width: 14, height: 14, borderRadius: 4, flex: 'none', display: 'grid', placeItems: 'center',
                    border: `1.5px solid ${worktree ? ACCENT : 'var(--faint)'}`,
                    background: worktree ? ACCENT : 'transparent',
                    color: 'oklch(.99 .01 160)', fontSize: 11, lineHeight: 1
                  }}
                >
                  {worktree ? '✓' : ''}
                </span>
                <span style={{ fontSize: 13.5, fontWeight: 500, whiteSpace: 'nowrap' }}>Git worktree</span>
              </div>
            </div>
          </div>

          <div>
            <div style={labelStyle}>Permission mode</div>
            <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
              {(Object.keys(PERM_HINTS) as PermMode[]).map(p => {
                const on = perm === p;
                return (
                  <div
                    key={p}
                    onClick={() => setPerm(p)}
                    style={{
                      display: 'flex', alignItems: 'center', gap: 9, padding: '7px 9px',
                      border: `1px solid ${on ? ACCENT : 'var(--line)'}`, borderRadius: 8, cursor: 'default',
                      background: on ? tint(11, 'transparent') : 'transparent'
                    }}
                  >
                    <span style={{ width: 13, height: 13, borderRadius: '50%', border: `1.5px solid ${on ? ACCENT : 'var(--faint)'}`, display: 'grid', placeItems: 'center', flex: 'none' }}>
                      <span style={{ width: 7, height: 7, borderRadius: '50%', background: on ? ACCENT : 'transparent' }} />
                    </span>
                    <span style={{ fontSize: 13.5, fontWeight: 500, whiteSpace: 'nowrap' }}>{p}</span>
                    <span style={{ fontSize: 12, color: 'var(--faint)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{PERM_HINTS[p]}</span>
                  </div>
                );
              })}
            </div>
          </div>
        </div>

        <div style={{ padding: '12px 18px', borderTop: '1px solid var(--line)', display: 'flex', justifyContent: 'flex-end', gap: 8, background: 'var(--panel)' }}>
          <div
            onClick={onClose}
            className="hover-bg"
            style={{ height: 31, padding: '0 14px', borderRadius: 8, border: '1px solid var(--line)', background: 'var(--bg)', display: 'flex', alignItems: 'center', fontSize: 13.5, cursor: 'default' }}
          >
            Cancel
          </div>
          <div
            onClick={() => void create()}
            className="hover-bright"
            style={{
              height: 31, padding: '0 16px', borderRadius: 8, background: 'var(--accent)', color: 'oklch(.99 .01 160)',
              display: 'flex', alignItems: 'center', fontSize: 13.5, fontWeight: 590, cursor: 'default',
              opacity: canCreate && !creating ? 1 : 0.5
            }}
          >
            {creating ? 'Trusting…' : 'Create'}
          </div>
        </div>
      </div>
    </div>
  );
}
