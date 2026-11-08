import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from 'react';
import { createPortal } from 'react-dom';
import { SYSTEM_BROWSER, type BrowserDto, type BrowserSettingsDto } from '../../ipc/browsers';
import { useAccounts } from '../accounts/accounts.store';
import { useChats } from '../chats/chats.store';
import { PROVIDER_LABEL } from '../../shared/types';
import { useDialogHost } from '../../shared/ui/dialogHost';
import { CommandLine, focusables, Group, List, ListItem, Row } from '../settings/SettingsRows';
import { choices, useBrowsers } from './browsers.store';

/**
 * Settings → Links: which browser a terminal's links open in. The default,
 * then the accounts and project folders that take another, and the browsers
 * to pick from — those found on this machine, one per profile, and the
 * user's own, any program with any flags.
 */

/** The browsers and a saver that reports what the Rust side refused. */
function useLinks() {
  const data = useBrowsers(s => s.data);
  const [error, setError] = useState<string | null>(null);
  const save = (next: BrowserSettingsDto) => {
    setError(null);
    void useBrowsers
      .getState()
      .save(next)
      .catch(e => setError(String(e)));
  };
  return { data, save, error };
}

function PickBrowser({
  value,
  all,
  label,
  onChange
}: {
  value: string;
  all: BrowserDto[];
  label: string;
  onChange: (id: string) => void;
}) {
  return (
    <select value={value} onChange={e => onChange(e.target.value)} className="set-browser" aria-label={label}>
      {all.map(b => (
        <option key={b.id} value={b.id}>
          {b.name}
        </option>
      ))}
      {!all.some(b => b.id === value) && (
        <option value={value}>{`${value} (gone; the system default stands in)`}</option>
      )}
    </select>
  );
}

type Kind = 'accounts' | 'folders';

export function OpenLinksSection() {
  const { data, save, error } = useLinks();
  const accounts = useAccounts(s => s.accounts);
  const folders = useChats(s => s.folders);

  // Loaded each time Settings opens: a browser installed meanwhile shows up.
  useEffect(() => {
    void useBrowsers.getState().load();
  }, []);

  const about = {
    summary: 'How a chat’s browser is chosen',
    body: (
      <p>
        A chat’s links open in the browser it was told to remember (Shift+click on a link to pick one, and tick
        “Remember for this chat”), else its account’s, else its folder’s, else the one above.
      </p>
    )
  };
  if (!data)
    return (
      <Group title="Open links in" about={about}>
        <Row desc="Looking for browsers…" />
      </Group>
    );

  const settings = data.settings;
  const all = choices(data);
  const accountLabel = (key: string) => {
    const a = accounts.find(x => `${x.provider}/${x.name}` === key);
    return a ? `${PROVIDER_LABEL[a.provider]} · ${a.name}` : key;
  };
  const rows = [
    ...Object.entries(settings.accounts).map(([key, id]) => ({ kind: 'accounts' as const, key, id })),
    ...Object.entries(settings.folders).map(([key, id]) => ({ kind: 'folders' as const, key, id }))
  ];
  const put = (kind: Kind, map: Record<string, string>) => save({ ...settings, [kind]: map });
  const free = {
    accounts: accounts
      .map(a => `${a.provider}/${a.name}`)
      .filter(k => !(k in settings.accounts))
      .map(key => ({ key, label: accountLabel(key) })),
    folders: folders
      .map(f => f.path)
      .filter(k => !(k in settings.folders))
      .map(key => ({ key, label: key }))
  };

  return (
    <Group title="Open links in" about={about}>
      <Row
        label="Default browser"
        desc="Unless a chat, its account or its folder has a browser of its own."
        control={
          <span className="set-under-remove">
            <PickBrowser
              all={all}
              label="Default browser"
              value={settings.default ?? SYSTEM_BROWSER}
              onChange={v => save({ ...settings, default: v === SYSTEM_BROWSER ? null : v })}
            />
          </span>
        }
      />
      <Row
        label="Exceptions"
        desc="Accounts and project folders whose links open in another browser."
        below={
          <>
            <List
              label="Exceptions"
              empty="No exceptions. Every chat uses the default browser."
              items={rows.map(r => {
                const label = r.kind === 'accounts' ? accountLabel(r.key) : r.key;
                return (
                  <ListItem key={`${r.kind}:${r.key}`}>
                    <span className="set-item-kind">{r.kind === 'accounts' ? 'Account' : 'Folder'}</span>
                    <span className="set-item-main">
                      <span className="set-item-name" title={r.key}>
                        {label}
                      </span>
                    </span>
                    <PickBrowser
                      all={all}
                      label={`Browser for ${label}`}
                      value={r.id}
                      onChange={v => put(r.kind, { ...settings[r.kind], [r.key]: v })}
                    />
                    <button
                      className="slim set-remove"
                      title="Back to the default"
                      aria-label={`Remove the exception for ${label}`}
                      onClick={() => {
                        const next = { ...settings[r.kind] };
                        delete next[r.key];
                        put(r.kind, next);
                      }}
                    >
                      Remove
                    </button>
                  </ListItem>
                );
              })}
              actions={
                <AddException
                  free={free}
                  onPick={(kind, key) => put(kind, { ...settings[kind], [key]: settings.default ?? SYSTEM_BROWSER })}
                />
              }
            />
            {error && <div className="set-error">{error}</div>}
          </>
        }
      />
    </Group>
  );
}

/** "Add an exception…": a menu of the accounts and project folders that have
 *  no browser of their own yet. */
function AddException({
  free,
  onPick
}: {
  free: Record<Kind, { key: string; label: string }[]>;
  onPick: (kind: Kind, key: string) => void;
}) {
  const host = useDialogHost();
  const face = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  const none = !free.accounts.length && !free.folders.length;

  useLayoutEffect(() => {
    if (!open || !face.current) return;
    const r = face.current.getBoundingClientRect();
    const h = menu.current?.offsetHeight ?? 200;
    const below = r.bottom + 4;
    setPos({ left: r.left, top: below + h > window.innerHeight - 8 ? Math.max(8, r.top - h - 4) : below });
    menu.current?.querySelector<HTMLElement>('[role="menuitem"]')?.focus();
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const away = (e: MouseEvent) => {
      const t = e.target as Node;
      if (!menu.current?.contains(t) && !face.current?.contains(t)) setOpen(false);
    };
    // Capture, ahead of Settings' own Escape, which would close it too.
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      e.preventDefault();
      e.stopPropagation();
      setOpen(false);
      face.current?.focus();
    };
    document.addEventListener('mousedown', away);
    window.addEventListener('keydown', onKey, true);
    return () => {
      document.removeEventListener('mousedown', away);
      window.removeEventListener('keydown', onKey, true);
    };
  }, [open]);

  // A menu as the ARIA pattern has it: its items are out of the tab order
  // and the arrows, Home and End move between them. Tab and Shift+Tab close
  // it and go on from its button, the way they would had it never opened
  // (the menu itself is portaled to the end of the page, where Tab would
  // otherwise leave the dialog).
  const walk = (e: ReactKeyboardEvent) => {
    if (e.key === 'Tab') {
      e.preventDefault();
      e.stopPropagation();
      setOpen(false);
      const button = face.current;
      const box = button?.closest('[role="dialog"]');
      const all = box ? focusables(box) : [];
      const at = button ? all.indexOf(button) : -1;
      (e.shiftKey || at < 0 ? button : (all[at + 1] ?? all[0]))?.focus();
      return;
    }
    const items = [...(menu.current?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? [])];
    const at = items.indexOf(document.activeElement as HTMLElement);
    const to =
      e.key === 'ArrowDown'
        ? (at + 1) % items.length
        : e.key === 'ArrowUp'
          ? (at - 1 + items.length) % items.length
          : e.key === 'Home'
            ? 0
            : e.key === 'End'
              ? items.length - 1
              : null;
    if (to === null) return;
    e.preventDefault();
    items[to]?.focus();
  };

  const pick = (kind: Kind, key: string) => {
    setOpen(false);
    onPick(kind, key);
    face.current?.focus();
  };

  return (
    <>
      <button
        ref={face}
        className="slim"
        aria-haspopup="menu"
        aria-expanded={open}
        disabled={none}
        title={none ? 'Every account and project folder has a browser of its own' : undefined}
        onClick={() => setOpen(o => !o)}
      >
        Add an exception…
      </button>
      {open &&
        host &&
        createPortal(
          <div
            ref={menu}
            role="menu"
            aria-label="Add an exception"
            className="popover set-menu"
            style={{ left: pos?.left ?? -9999, top: pos?.top ?? 0 }}
            onKeyDown={walk}
          >
            {(['accounts', 'folders'] as const).map(
              kind =>
                free[kind].length > 0 && (
                  <div key={kind} role="group" aria-label={kind === 'accounts' ? 'Accounts' : 'Project folders'}>
                    <div className="set-menu-head">{kind === 'accounts' ? 'Accounts' : 'Project folders'}</div>
                    {free[kind].map(k => (
                      <button key={k.key} role="menuitem" tabIndex={-1} title={k.key} onClick={() => pick(kind, k.key)}>
                        {k.label}
                      </button>
                    ))}
                  </div>
                )
            )}
          </div>,
          host
        )}
    </>
  );
}

export function BrowsersSection() {
  const { data, save, error } = useLinks();
  if (!data) return null;
  const settings = data.settings;
  return (
    <Group
      title="Browsers"
      about={{
        summary: 'How browsers are set up',
        body: (
          <p>
            A browser is a command line with <code>%1</code> where the link goes, so a browser profile is a flag:
            Chrome, Edge and Brave take <code>--profile-directory="Profile 1"</code>, Firefox <code>-P name</code>. The
            link is passed as one argument of its own, never through a shell.
          </p>
        )
      }}
    >
      <Row
        desc="Found on this computer, one per profile, and any you add."
        below={
          <>
            <List
              label="Browsers"
              empty="Only the system default was found."
              extra={<CommandLine name="System default" command={data.system ?? ''} />}
              items={[
                ...data.found.map(b => <CommandLine key={b.id} name={b.name} command={b.command} />),
                ...settings.custom.map((b, i) => (
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
                ))
              ]}
              actions={
                <button
                  className="slim"
                  onClick={() => {
                    const base = choices(data).find(b => b.id === (settings.default ?? SYSTEM_BROWSER));
                    const id = `custom:${Date.now().toString(36)}`;
                    save({
                      ...settings,
                      custom: [
                        ...settings.custom,
                        { id, name: 'My browser', command: base?.command || '"C:\\path\\browser.exe" %1' }
                      ]
                    });
                  }}
                >
                  Add a browser
                </button>
              }
            />
            {error && <div className="set-error">{error}</div>}
          </>
        }
      />
    </Group>
  );
}

/** One of the user's own browsers: its name and command, kept on Save. */
function CustomRow(props: {
  name: string;
  command: string;
  onSave: (name: string, command: string) => void;
  onRemove: () => void;
}) {
  const [name, setName] = useState(props.name);
  const [command, setCommand] = useState(props.command);
  const changed = name !== props.name || command !== props.command;
  return (
    <ListItem>
      <input
        type="text"
        value={name}
        onChange={e => setName(e.target.value)}
        style={{ flex: 'none', width: 150 }}
        aria-label="Browser name"
      />
      <input
        type="text"
        value={command}
        onChange={e => setCommand(e.target.value)}
        className="set-mono"
        style={{ flex: 1, minWidth: 0 }}
        aria-label="Browser command line"
      />
      <span className="set-item-actions">
        <button
          className="slim"
          disabled={!changed || !name.trim() || !command.trim()}
          onClick={() => props.onSave(name.trim(), command.trim())}
        >
          Save
        </button>
        <button
          className="slim"
          aria-label={`Remove ${props.name}`}
          title="Remove this browser"
          onClick={props.onRemove}
        >
          Remove
        </button>
      </span>
    </ListItem>
  );
}
