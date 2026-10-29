import { useEffect, useState } from 'react';
import { libraryDiff, type FileDiffDto } from '../../ipc/commands';
import { diffLines, hunks } from './diff';

const ADDED = 'color-mix(in oklab, oklch(.68 .16 145) 22%, transparent)';
const TAKEN = 'color-mix(in oklab, oklch(.63 .19 25) 20%, transparent)';

function Changes({ label, from, to }: { label: string; from: string; to: string }) {
  return (
    <div style={{ marginTop: 6 }}>
      <div style={{ fontSize: 'var(--fs-1)', color: 'var(--faint)', marginBottom: 2 }}>{label}</div>
      <div className="library-diff">
        {hunks(diffLines(from, to)).map((h, i) =>
          h.kind === 'skip' ? (
            <div key={i} style={{ color: 'var(--faint)', fontStyle: 'italic', padding: '0 8px' }}>
              … {h.count} unchanged line{h.count === 1 ? '' : 's'} …
            </div>
          ) : (
            h.lines.map((l, j) => (
              <div key={`${i}-${j}`} style={{ background: l.kind === '+' ? ADDED : l.kind === '-' ? TAKEN : undefined, padding: '0 8px' }}>
                <span style={{ color: 'var(--faint)', userSelect: 'none' }}>{l.kind} </span>
                {l.text || ' '}
              </div>
            ))
          )
        )}
      </div>
    </div>
  );
}

/** One file: what changed where, against what Luna wrote last when it knows that. */
function FileChanges({ f }: { f: FileDiffDto }) {
  const say = (text: string) => <div style={{ fontSize: 'var(--fs-2)', color: 'var(--dim)', marginTop: 4 }}>{text}</div>;
  let body;
  if (f.binary) body = say('Not a text file — it differs, line by line cannot be shown.');
  else if (f.base !== null) {
    body = (
      <>
        {f.there === null
          ? say('Deleted there.')
          : f.there !== f.base && <Changes label="There, since Luna wrote it" from={f.base} to={f.there} />}
        {f.library === null
          ? say('Not in the library any more.')
          : f.library !== f.base && <Changes label="In the library, since then" from={f.base} to={f.library} />}
      </>
    );
  } else if (f.library === null) body = say('Only there — the library has no such file.');
  else if (f.there === null) body = say('Only in the library.');
  else body = <Changes label="The library’s → what is there" from={f.library} to={f.there} />;
  return (
    <div style={{ marginTop: 8 }}>
      <div style={{ fontSize: 'var(--fs-2)', fontWeight: 700 }}>{f.path}</div>
      {body}
    </div>
  );
}

/** The differences behind a copy that needs a look, file by file. */
export function DiffView({ dest }: { dest: string }) {
  const [files, setFiles] = useState<FileDiffDto[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    void libraryDiff(dest).then(setFiles).catch(e => setError(String(e)));
  }, [dest]);
  if (error) return <div style={{ fontSize: 'var(--fs-2)', color: 'oklch(.58 .2 25)' }}>{error}</div>;
  if (!files) return <div style={{ fontSize: 'var(--fs-2)', color: 'var(--faint)' }}>Reading…</div>;
  if (!files.length) return <div style={{ fontSize: 'var(--fs-2)', color: 'var(--faint)' }}>No differences any more.</div>;
  return (
    <div style={{ padding: '2px 0 6px' }}>
      {files.map(f => (
        <FileChanges key={f.path} f={f} />
      ))}
    </div>
  );
}
