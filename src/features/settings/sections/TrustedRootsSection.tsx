import { useEffect, useState } from 'react';
import { pickFolder } from '../../../ipc/cli';
import { codexTrustedRoots, setCodexTrustedRoots } from '../../../ipc/trust';
import { Group, List, ListItem, Row } from '../SettingsRows';

/** The folders whose projects Codex sessions start trusted on this machine:
 *  Codex looks trust up by a checkout's exact path, so a parent folder in its
 *  own config covers nothing below it (codex/trust.rs). */
export function TrustedRootsSection() {
  const [roots, setRoots] = useState<string[] | null>(null);
  useEffect(() => {
    void codexTrustedRoots().then(setRoots);
  }, []);
  const save = (next: string[]) => void setCodexTrustedRoots(next).then(setRoots);
  return (
    <Group
      title="Trusted folders"
      about={{
        summary: 'How trusted folders work',
        body: (
          <p>
            A Codex session in a project anywhere under these starts trusted — its own <code>.codex/</code> config,
            hooks and rules load — with nothing written to the account’s <code>config.toml</code>. Codex itself trusts
            each project by its exact path only. Helpers in the background are trusted wherever they are sent.
          </p>
        )
      }}
    >
      <Row
        desc="A Codex session in a project under any of these folders starts trusted."
        below={
          <List
            label="Trusted folders"
            empty="No trusted folders. Codex asks in every project."
            items={(roots ?? []).map(r => (
              <ListItem key={r}>
                <span className="set-item-main">
                  <span className="set-item-name" title={r}>
                    {r}
                  </span>
                </span>
                <button className="slim set-remove" onClick={() => save((roots ?? []).filter(x => x !== r))}>
                  Remove
                </button>
              </ListItem>
            ))}
            actions={
              <button
                className="slim"
                disabled={roots === null}
                onClick={() =>
                  void pickFolder().then(p => {
                    if (p && roots && !roots.includes(p)) save([...roots, p]);
                  })
                }
              >
                Add folder…
              </button>
            }
          />
        }
      />
    </Group>
  );
}
