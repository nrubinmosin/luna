import { useEffect, useState } from 'react';
import { pickFolder } from '../../../ipc/cli';
import { codexTrustedRoots, setCodexTrustedRoots } from '../../../ipc/worktrees';
import { Section } from '../../../shared/ui/Section';

/** The folders whose projects Codex sessions start trusted on this machine:
 *  Codex looks trust up by a checkout's exact path, so a parent folder in its
 *  own config covers nothing below it (codex/trust.rs). */
function TrustedRoots() {
  const [roots, setRoots] = useState<string[] | null>(null);
  useEffect(() => {
    void codexTrustedRoots().then(setRoots);
  }, []);
  const save = (next: string[]) => void setCodexTrustedRoots(next).then(setRoots);
  return (
    <>
      {(roots ?? []).map(r => (
        <div key={r} className="field-row" style={{ padding: '2px 0', gap: 6 }}>
          <span
            title={r}
            style={{
              flex: 1,
              minWidth: 0,
              fontSize: 'var(--fs-3)',
              color: 'var(--dim)',
              overflow: 'hidden',
              textOverflow: 'ellipsis',
              whiteSpace: 'nowrap'
            }}
          >
            {r}
          </span>
          <button className="slim" onClick={() => save((roots ?? []).filter(x => x !== r))}>
            Remove
          </button>
        </div>
      ))}
      <button
        className="slim"
        disabled={roots === null}
        onClick={() =>
          void pickFolder().then(p => {
            if (p && roots && !roots.includes(p)) save([...roots, p]);
          })
        }
        style={{ marginTop: 4 }}
      >
        Add folder…
      </button>
      <div style={{ fontSize: 'var(--fs-1)', color: 'var(--faint)', marginTop: 4 }}>
        A Codex session in a project anywhere under these starts trusted — its own <code>.codex/</code> config, hooks
        and rules load — with nothing written to the account’s <code>config.toml</code>. Codex itself trusts each
        project by its exact path only. Helpers in the background are trusted wherever they are sent.
      </div>
    </>
  );
}

export function TrustedRootsSection() {
  return (
    <Section label="Codex trusts on this computer">
      <TrustedRoots />
    </Section>
  );
}
