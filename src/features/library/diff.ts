export type DiffLine = { kind: ' ' | '+' | '-'; text: string };

/** Past this many cells the table costs more than a picture is worth. */
const MAX_CELLS = 4_000_000;

/**
 * `a` turned into `b`, line by line: the longest run of lines the two share
 * stays, the rest is taken out of `a` and put in from `b`. The ends both have
 * are set aside first, so a small change in a long file costs a small table.
 */
export function diffLines(a: string, b: string): DiffLine[] {
  const x = a.split('\n');
  const y = b.split('\n');
  let start = 0;
  while (start < x.length && start < y.length && x[start] === y[start]) start++;
  let ex = x.length;
  let ey = y.length;
  while (ex > start && ey > start && x[ex - 1] === y[ey - 1]) {
    ex--;
    ey--;
  }
  const xs = x.slice(start, ex);
  const ys = y.slice(start, ey);
  const n = xs.length;
  const m = ys.length;
  const middle: DiffLine[] = [];
  if ((n + 1) * (m + 1) > MAX_CELLS) {
    for (const t of xs) middle.push({ kind: '-', text: t });
    for (const t of ys) middle.push({ kind: '+', text: t });
  } else {
    // lcs[i][j]: the longest common run of xs[i..] and ys[j..].
    const w = m + 1;
    const lcs = new Uint32Array((n + 1) * w);
    for (let i = n - 1; i >= 0; i--) {
      for (let j = m - 1; j >= 0; j--) {
        lcs[i * w + j] = xs[i] === ys[j] ? lcs[(i + 1) * w + j + 1] + 1 : Math.max(lcs[(i + 1) * w + j], lcs[i * w + j + 1]);
      }
    }
    let i = 0;
    let j = 0;
    while (i < n || j < m) {
      if (i < n && j < m && xs[i] === ys[j]) {
        middle.push({ kind: ' ', text: xs[i] });
        i++;
        j++;
      } else if (i < n && (j === m || lcs[(i + 1) * w + j] >= lcs[i * w + j + 1])) {
        // What goes out before what comes in, as diffs are read.
        middle.push({ kind: '-', text: xs[i++] });
      } else {
        middle.push({ kind: '+', text: ys[j++] });
      }
    }
  }
  return [
    ...x.slice(0, start).map(text => ({ kind: ' ' as const, text })),
    ...middle,
    ...x.slice(ex).map(text => ({ kind: ' ' as const, text }))
  ];
}

export type Hunk = { kind: 'lines'; lines: DiffLine[] } | { kind: 'skip'; count: number };

/** The changes with `context` unchanged lines around each; longer quiet runs fold. */
export function hunks(lines: DiffLine[], context = 3): Hunk[] {
  const out: Hunk[] = [];
  let run: DiffLine[] = [];
  const flush = (atStart: boolean, atEnd: boolean) => {
    const head = atStart ? 0 : context;
    const tail = atEnd ? 0 : context;
    if (run.length > head + tail + 1) {
      if (head) out.push({ kind: 'lines', lines: run.slice(0, head) });
      out.push({ kind: 'skip', count: run.length - head - tail });
      if (tail) out.push({ kind: 'lines', lines: run.slice(run.length - tail) });
    } else if (run.length) {
      out.push({ kind: 'lines', lines: run });
    }
    run = [];
  };
  let seenChange = false;
  for (const l of lines) {
    if (l.kind === ' ') {
      run.push(l);
      continue;
    }
    flush(!seenChange, false);
    seenChange = true;
    out.push({ kind: 'lines', lines: [l] });
  }
  flush(!seenChange, true);
  return out;
}
