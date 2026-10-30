import { useEffect, useState } from 'react';
import { SYSTEM_BROWSER, type BrowserSettingsDto } from '../../ipc/commands';
import { useAccounts } from '../accounts/accounts.store';
import { useChats } from '../chats/chats.store';
import { PROVIDER_LABEL } from '../../shared/types';
import { choices, useBrowsers } from './browsers.store';

const fine = { fontSize: 'var(--fs-1)', color: 'var(--faint)', marginTop: 4 } as const;
const rowStyle = { display: 'flex', gap: 6, alignItems: 'center', padding: '2px 0' } as const;

/**
 * Settings → Links: which browser a terminal's links open in. The default,
 * then the ones an account or a project folder takes instead, and the
 * browsers to pick from — those found on this machine, one per profile, and
 * the user's own, any program with any flags.
 */
export function BrowserSection() {
  const data = useBrowsers(s => s.data);
  const accounts = useAccounts(s => s.accounts);
  const folders = useChats(s => s.folders);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void useBrowsers.getState().load();
  }, []);

  if (!data) return <div style={fine}>Looking for browsers…</div>;
  const settings = data.settings;
  const all = choices(data);
  const nameOf = (id: string) => all.find(b => b.id === id)?.name ?? `${id} (gone; the system default stands in)`;
  const save = (next: BrowserSettingsDto) => {
    setError(null);
    void useBrowsers.getState().save(next).catch(e => setError(String(e)));
  };

  const pickBrowser = (value: string, onChange: (id: string) => void) => (
    <select value={value} onChange={e => onChange(e.target.value)} style={{ flex: 1, minWidth: 0 }}>
      {all.map(b => (
        <option key={b.id} value={b.id}>
          {b.name}
        </option>
      ))}
      {!all.some(b => b.id === value) && <option value={value}>{nameOf(value)}</option>}
    </select>
  );

  const accountKeys = accounts.map(a => ({ key: `${a.provider}/${a.name}`, label: `${PROVIDER_LABEL[a.provider]} · ${a.name}` }));
  const folderKeys = folders.map(f => ({ key: f.path, label: f.path }));

  const overrides = (
    title: string,
    map: Record<string, string>,
    keys: { key: string; label: string }[],
    put: (map: Record<string, string>) => void
  ) => {
    const free = keys.filter(k => !(k.key in map));
    return (
      <div style={{ marginTop: 8 }}>
        <div style={{ fontSize: 'var(--fs-3)', color: 'var(--dim)', fontWeight: 600 }}>{title}</div>
        {Object.entries(map).map(([key, id]) => (
          <div key={key} style={rowStyle}>
            <span style={{ flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }} title={key}>
              {keys.find(k => k.key === key)?.label ?? key}
            </span>
            {pickBrowser(id, v => put({ ...map, [key]: v }))}
            <button
              aria-label="Remove"
              title="Back to the default"
              onClick={() => {
                const next = { ...map };
                delete next[key];
                put(next);
              }}
            >
              ✕
            </button>
          </div>
        ))}
        {free.length > 0 && (
          <div style={rowStyle}>
            <select value="" onChange={e => e.target.value && put({ ...map, [e.target.value]: settings.default ?? SYSTEM_BROWSER })} style={{ flex: 1, minWidth: 0 }}>
              <option value="">Add…</option>
              {free.map(k => (
                <option key={k.key} value={k.key}>
                  {k.label}
                </option>
              ))}
            </select>
          </div>
        )}
      </div>
    );
  };

  return (
    <div>
      <div style={rowStyle}>
        <span style={{ fontSize: 'var(--fs-4)' }}>Open links in</span>
        {pickBrowser(settings.default ?? SYSTEM_BROWSER, v => save({ ...settings, default: v === SYSTEM_BROWSER ? null : v }))}
      </div>
      {overrides('For an account', settings.accounts, accountKeys, accountsMap => save({ ...settings, accounts: accountsMap }))}
      {overrides('For a project folder', settings.folders, folderKeys, foldersMap => save({ ...settings, folders: foldersMap }))}

      <div style={{ marginTop: 8 }}>
        <div style={{ fontSize: 'var(--fs-3)', color: 'var(--dim)', fontWeight: 600 }}>Browsers</div>
        {[{ id: SYSTEM_BROWSER, name: 'System default', command: data.system ?? '' }, ...data.found].map(b => (
          <div key={b.id} style={{ padding: '2px 0' }}>
            <div style={{ fontSize: 'var(--fs-4)' }}>{b.name}</div>
            {b.command && <div style={{ ...fine, marginTop: 0, wordBreak: 'break-all' }}><code>{b.command}</code></div>}
          </div>
        ))}
        {settings.custom.map((b, i) => (
          <CustomRow
            key={b.id}
            name={b.name}
            command={b.command}
            onSave={(name, command) => {
              const custom = [...settings.custom];
              custom[i] = { ...b, name, command };
              save({ ...settings, custom });
            }}
            onRemove={() => save({ ...settings, custom: settings.custom.filter(c => c.id !== b.id) })}
          />
        ))}
        <button
          style={{ marginTop: 4 }}
          onClick={() => {
            const base = all.find(b => b.id === (settings.default ?? SYSTEM_BROWSER));
            const id = `custom:${Date.now().toString(36)}`;
            save({ ...settings, custom: [...settings.custom, { id, name: 'My browser', command: base?.command || '"C:\\path\\browser.exe" %1' }] });
          }}
        >
          Add a browser
        </button>
      </div>
      {error && <div style={{ ...fine, color: 'var(--danger, #c33)' }}>{error}</div>}
      <div style={fine}>
        A chat’s links open in the browser it was told to remember (Shift+click on a link to pick one, and
        tick “Remember for this chat”), else its account’s, else its folder’s, else the one above. A
        browser is a command line with <code>%1</code> where the link goes, so a browser profile is a flag:
        Chrome, Edge and Brave take <code>--profile-directory="Profile 1"</code>, Firefox{' '}
        <code>-P name</code>. The link is passed as one argument of its own, never through a shell.
      </div>
    </div>
  );
}

/** One of the user's own browsers: its name and command, kept on Save. */
function CustomRow(props: { name: string; command: string; onSave: (name: string, command: string) => void; onRemove: () => void }) {
  const [name, setName] = useState(props.name);
  const [command, setCommand] = useState(props.command);
  const changed = name !== props.name || command !== props.command;
  return (
    <div style={{ padding: '3px 0' }}>
      <div style={rowStyle}>
        <input type="text" value={name} onChange={e => setName(e.target.value)} style={{ flex: 1, minWidth: 0 }} aria-label="Name" />
        <button disabled={!changed || !name.trim() || !command.trim()} onClick={() => props.onSave(name.trim(), command.trim())}>
          Save
        </button>
        <button aria-label="Remove" title="Remove this browser" onClick={props.onRemove}>
          ✕
        </button>
      </div>
      <input
        type="text"
        value={command}
        onChange={e => setCommand(e.target.value)}
        style={{ width: '100%', fontFamily: 'var(--mono, monospace)', fontSize: 'var(--fs-1)' }}
        aria-label="Command"
      />
    </div>
  );
}
