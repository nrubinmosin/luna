import { useEffect, useRef, useState } from 'react';
import type { CSSProperties } from 'react';
import { EFFORTS, MODELS, PERM_HINTS } from '../../shared/types';
import type { Effort, ModelLabel, PermMode } from '../../shared/types';
import { ACCENT, tail2, tint } from '../../shared/lib/format';
import { useChats } from '../chats/chats.store';
import { useAccounts } from '../accounts/accounts.store';
import { useNewChat } from './newchat.store';
import { createChat, settingsDefaults, SOURCE_LABELS, STOCK, type ChatSettings, type SettingsFrom } from './create';
import { folderTrusted, pickFolder } from '../../ipc/commands';

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

  const [folder, setFolder] = useState(
    () => initialFolder ?? useNewChat.getState().lastFolder ?? folders[0]?.path ?? ''
  );
  const [account, setAccount] = useState(() => {
    const last = useNewChat.getState().lastAccount;
    return (last && accounts.some(a => a.name === last) ? last : accounts[0]?.name) ?? '';
  });
  const [worktree, setWorktree] = useState(() => useNewChat.getState().lastWorktree);

  // What the settings files say, and what this chat will actually open on —
  // the same thing until something here is changed by hand.
  const [resolved, setResolved] = useState<ChatSettings>(STOCK);
  const [from, setFrom] = useState<SettingsFrom>({});
  const [settings, setSettings] = useState<ChatSettings>(STOCK);
  // A control the user has touched is theirs: re-resolving after a change of
  // account or folder must not quietly take it back. In a ref because the
  // resolver should not re-run just because something was touched.
  const touched = useRef<Partial<Record<keyof ChatSettings, true>>>({});

  const canCreate = !!folder && !!account;
  const accountPath = accounts.find(a => a.name === account)?.path ?? '';

  useEffect(() => {
    let stale = false;
    void settingsDefaults(accountPath, folder).then(({ values, from: sources }) => {
      if (stale) return;
      setResolved(values);
      setFrom(sources);
      setSettings(prev => ({
        model: touched.current.model ? prev.model : values.model,
        effort: touched.current.effort ? prev.effort : values.effort,
        perm: touched.current.perm ? prev.perm : values.perm
      }));
    });
    return () => {
      stale = true;
    };
  }, [accountPath, folder]);

  // The CLI can't show its trust prompt under --worktree; it just tells you to
  // open the folder without isolation first. Detect the untrusted case up front
  // so creating the chat can write the same bit the prompt would.
  const [trusted, setTrusted] = useState(true);
  const [creating, setCreating] = useState(false);
  const [trustError, setTrustError] = useState<string | null>(null);
  useEffect(() => {
    let stale = false;
    if (!folder || !accountPath) {
      setTrusted(true);
      return;
    }
    void folderTrusted(accountPath, folder)
      .then(ok => !stale && setTrusted(ok))
      .catch(() => !stale && setTrusted(true));
    return () => {
      stale = true;
    };
  }, [folder, accountPath]);

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
    setCreating(true);
    try {
      await createChat({ folder, account, ...settings, worktree });
    } catch (e) {
      setCreating(false);
      setTrustError(String(e));
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
      void create();
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  });

  const pick = <K extends keyof ChatSettings>(key: K, value: ChatSettings[K]) => {
    touched.current[key] = true;
    setSettings(s => ({ ...s, [key]: value }));
  };
  const revert = <K extends keyof ChatSettings>(key: K) => {
    delete touched.current[key];
    setSettings(s => ({ ...s, [key]: resolved[key] }));
  };

  const labelStyle: CSSProperties = { fontSize: 'var(--fs-3)', color: 'var(--dim)', marginBottom: 4, fontWeight: 600 };
  const noteStyle: CSSProperties = { fontSize: 'var(--fs-2)', color: 'var(--faint)', marginTop: 3 };

  /** Where the value on screen came from, or the way back if it was changed.
   *  A plain call rather than a component: a component declared in here is a
   *  new type on every render, and React would tear the node down and build
   *  it again each time. */
  const origin = (field: keyof ChatSettings) => {
    if (settings[field] !== resolved[field]) {
      return (
        <div style={noteStyle}>
          just for this chat —{' '}
          <span
            onClick={() => revert(field)}
            className="hover-bg"
            style={{ cursor: 'default', color: 'var(--dim)', textDecoration: 'underline', borderRadius: 2 }}
          >
            back to {resolved[field]}
          </span>
        </div>
      );
    }
    const src = from[field];
    return (
      <div style={noteStyle}>
        {src ? `from ${SOURCE_LABELS[src]}` : 'nothing in settings says — Luna’s own default'}
      </div>
    );
  };

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
            Opens on this account’s Claude Code settings; anything changed here applies to this chat only
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
            {!trusted && folder && (
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
            <Segmented items={MODELS} value={settings.model} onPick={(v: ModelLabel) => pick('model', v)} />
            {origin('model')}
          </div>

          <div>
            <div style={labelStyle}>Effort</div>
            <Segmented items={EFFORTS} value={settings.effort} onPick={(v: Effort) => pick('effort', v)} />
            {origin('effort')}
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
                const on = settings.perm === p;
                const id = `perm-${p}`;
                return (
                  <div key={p} className="field-row">
                    <input
                      type="radio"
                      id={id}
                      name="perm-mode"
                      checked={on}
                      onChange={() => pick('perm', p)}
                    />
                    <label htmlFor={id} style={{ fontWeight: 600, whiteSpace: 'nowrap', cursor: 'default' }}>
                      {p}
                    </label>
                    <span style={{ fontSize: 'var(--fs-2)', color: 'var(--faint)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{PERM_HINTS[p]}</span>
                  </div>
                );
              })}
            </div>
            {origin('perm')}
          </div>

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
