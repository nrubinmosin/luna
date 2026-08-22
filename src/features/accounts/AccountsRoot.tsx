import { useEffect, useState } from 'react';
import { getAccountsRoot, pickFolder, setAccountsRoot, type AccountsRootInfo } from '../../ipc/commands';
import { useAccounts } from './accounts.store';

/**
 * Where the account folders live, and the way to move the list elsewhere —
 * onto the drive Luna itself sits on, typically. Folds out under the Accounts
 * header on demand; the path is furniture the rest of the time.
 *
 * Changing it repoints the list, nothing more: folders under the old root are
 * left alone, and chats find their account by name, so moving the folders by
 * hand afterwards (or before) is all it takes.
 */
export function AccountsRoot({ onClose }: { onClose: () => void }) {
  const [info, setInfo] = useState<AccountsRootInfo | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void getAccountsRoot().then(setInfo);
  }, []);

  const apply = async (path: string) => {
    setError(null);
    try {
      setInfo(await setAccountsRoot(path));
      await useAccounts.getState().refresh();
    } catch (e) {
      setError(String(e));
    }
  };

  const change = async () => {
    const picked = await pickFolder();
    if (picked) await apply(picked);
  };

  return (
    <div style={{ marginBottom: 8, padding: '5px 7px', borderRadius: 2, background: 'var(--chip)' }}>
      <div style={{ fontSize: 'var(--fs-1)', color: 'var(--faint)', textTransform: 'uppercase', letterSpacing: '.04em', fontWeight: 700, marginBottom: 3 }}>
        Accounts folder
      </div>
      <div
        title={info?.path}
        style={{
          fontSize: 'var(--fs-2)', color: 'var(--dim)', whiteSpace: 'nowrap', overflow: 'hidden',
          textOverflow: 'ellipsis', direction: 'rtl', textAlign: 'left', marginBottom: 5
        }}
      >
        {/* rtl so the ellipsis eats the drive letter, not the folder name. */}
        <bdi>{info?.path ?? '…'}</bdi>
      </div>
      <div style={{ display: 'flex', gap: 4 }}>
        <button onClick={() => void change()} className="slim primary" style={{ flex: 1 }}>
          Change…
        </button>
        <button
          onClick={() => void apply('')}
          disabled={!info || info.isDefault}
          title="Back to Documents/claude-accounts"
          className="slim"
        >
          Default
        </button>
        <button onClick={onClose} className="slim" style={{ width: 26 }} aria-label="Close">
          ✕
        </button>
      </div>
      <div style={{ fontSize: 'var(--fs-1)', color: 'var(--faint)', marginTop: 4 }}>
        Only the location changes — move the account folders yourself. Running sessions keep their current paths.
      </div>
      {error && <div style={{ fontSize: 'var(--fs-2)', color: 'oklch(.58 .2 25)', marginTop: 4 }}>{error}</div>}
    </div>
  );
}
