import { useEffect, useState } from 'react';
import { getAccountsRoot, setAccountsRoot, type AccountsRootInfo } from '../../../ipc/accounts';
import { pickFolder } from '../../../ipc/cli';
import { useAccounts } from '../../accounts/accounts.store';
import { Group, Row } from '../SettingsRows';

/** The accounts-folder mover. What moving it leaves to you is short enough,
 *  and costly enough to miss, to stay in view rather than fold away. */
export function AccountsFolderSection() {
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
    <Group title="Accounts">
      <Row
        label="Accounts folder"
        desc={
          <span className="set-path" title={info?.path}>
            <bdi>{info?.path ?? '…'}</bdi>
          </span>
        }
        note={
          <span>
            Claude Code accounts live under <code>anthropic/</code>, Codex accounts under <code>openai/</code>. Only the
            location changes — move the account folders yourself. Running sessions keep their current paths.
          </span>
        }
        control={
          <>
            <button
              className="slim"
              onClick={() =>
                void pickFolder().then(p => {
                  if (p) return apply(p);
                })
              }
            >
              Change…
            </button>
            <button
              className="slim"
              onClick={() => void apply('')}
              disabled={!info || info.isDefault}
              title="Back to Documents/luna-accounts"
            >
              Default
            </button>
          </>
        }
        below={error && <div className="set-error">{error}</div>}
      />
    </Group>
  );
}
