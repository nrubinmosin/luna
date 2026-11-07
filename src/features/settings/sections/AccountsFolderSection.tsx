import { useEffect, useState } from 'react';
import { getAccountsRoot, setAccountsRoot, type AccountsRootInfo } from '../../../ipc/accounts';
import { pickFolder } from '../../../ipc/cli';
import { useAccounts } from '../../accounts/accounts.store';
import { Section } from '../../../shared/ui/Section';

/** The accounts-folder mover. */
function AccountsFolder() {
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

  return (
    <>
      <div
        title={info?.path}
        style={{
          fontSize: 'var(--fs-3)',
          color: 'var(--dim)',
          whiteSpace: 'nowrap',
          overflow: 'hidden',
          textOverflow: 'ellipsis',
          direction: 'rtl',
          textAlign: 'left',
          marginBottom: 6
        }}
      >
        {/* rtl so the ellipsis eats the drive letter, not the folder name. */}
        <bdi>{info?.path ?? '…'}</bdi>
      </div>
      <div style={{ display: 'flex', gap: 6 }}>
        <button
          onClick={() =>
            void pickFolder().then(p => {
              if (p) return apply(p);
            })
          }
          className="slim"
          style={{ minWidth: 74 }}
        >
          Change…
        </button>
        <button
          onClick={() => void apply('')}
          disabled={!info || info.isDefault}
          title="Back to Documents/luna-accounts"
          className="slim"
        >
          Default
        </button>
      </div>
      <div style={{ fontSize: 'var(--fs-1)', color: 'var(--faint)', marginTop: 5 }}>
        Claude Code accounts live under <code>anthropic/</code>, Codex accounts under <code>openai/</code>. Only the
        location changes — move the account folders yourself. Running sessions keep their current paths.
      </div>
      {error && <div style={{ fontSize: 'var(--fs-2)', color: 'oklch(.58 .2 25)', marginTop: 4 }}>{error}</div>}
    </>
  );
}

export function AccountsFolderSection() {
  return (
    <Section label="Accounts folder">
      <AccountsFolder />
    </Section>
  );
}
