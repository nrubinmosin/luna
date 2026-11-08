// Keeps the repository in US English. Every tracked text file is read line by
// line, and two things are reported as `path:line:col message`, exiting 1 on any:
//
// - a letter outside the Latin script (Cyrillic, Greek, CJK and the like;
//   Latin letters with diacritics, digits, punctuation and emoji pass);
// - a British spelling from the lists below, with the American one to use.
//
// Some text has to stay as it is: a test fixture that exercises non-Latin
// input, or another tool's literal output that happens to be British. Such a
// line carries the marker `lang-check: allow` as a comment of its own (after
// `//`, `#`, `<!--`, `/*`, `--` or a doc comment's `*`), at the end of the line
// or alone on the line just above it, and is skipped. A trailing marker covers
// its own line only, so the prose after it is still checked; the marker inside
// a string, or with anything but a comment's close after it, is no marker.
//
// A file of a known text type that is not UTF-8 is a finding as well; a file
// of another type that is not is taken for binary and passed over. A symbolic
// link is passed over too: git keeps it as the path it points at, not as text
// of its own, and on disk it may stand for a directory (a skill's
// `.claude/skills/<name>`). So is a directory named as a file.
//
//   node scripts/check-text.mjs            check the files git tracks
//   node scripts/check-text.mjs FILE...    check just these files

import { execFileSync } from 'node:child_process';
import { lstatSync, readFileSync } from 'node:fs';
import { basename, extname, isAbsolute, relative, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

export const MARKER = 'lang-check: allow';
const MARKER_TAIL = String.raw`\s*lang-check: allow\s*(?:-->|\*\/)?\s*$`;
// The marker ending a line, after a comment opener that starts the line or follows a space.
const MARKER_END = new RegExp(String.raw`(?:^\s*\*|(?:^|\s)(?:\/\/+|#|<!--|\/\*+|--))` + MARKER_TAIL);
// The marker alone on its line, which covers the line below.
const MARKER_LINE = new RegExp(String.raw`^\s*(?:\/\/+|#|<!--|\/\*+|\*|--)` + MARKER_TAIL);

/** The repository's root: every path is taken relative to it. */
export const ROOT = fileURLToPath(new URL('..', import.meta.url));

// Not text anyone writes (lockfiles, generated schemas), and this checker and
// its tests, which spell out the very words they look for.
const SKIP_PATHS = [/(^|\/)pnpm-lock\.yaml$/, /(^|\/)Cargo\.lock$/, /^src-tauri\/gen\//];
const SKIP_FILES = new Set(['scripts/check-text.mjs', 'scripts/check-text.test.mjs']);

// A file's type is its extension, or its name when it has none (`.gitignore`).
const fileType = path => (extname(path) || basename(path)).toLowerCase();
const BINARY_TYPES = new Set(
  '.png .ico .icns .jpg .jpeg .gif .webp .bmp .pdf .woff .woff2 .ttf .otf .zip .gz .exe .dll .wasm'.split(' ')
);
// Types that must be UTF-8 text.
const TEXT_TYPES = new Set(
  `.rs .ts .tsx .js .mjs .cjs .json .md .css .html .svg .xml .toml .yaml .yml .txt .ps1 .sh .nsh
   .dockerfile .editorconfig .gitattributes .gitignore .prettierignore`.split(/\s+/)
);

// A letter (\p{L}) that is not in the Latin script.
const NON_LATIN_LETTER = /(?=\p{L})\P{Script=Latin}/gu;

// British → American. Each key is a regular expression for the whole word with
// its inflections, matched case-insensitively at word boundaries, so a word
// glued into an identifier (`colour_scheme`, `initialiseFoo`) is not one.
const BRITISH = {
  '(?:re|dis)?colour(?:s|ed|ing|ful|less)?': 'color',
  'behaviours?|behavioural(?:ly)?': 'behavior',
  'flavours?|flavoured': 'flavor',
  'favours?|favoured|favouring|favourites?|favourable': 'favor',
  'honours?|honoured|honouring': 'honor',
  'neighbours?|neighbouring': 'neighbor',
  'labours?|laboured|labouring': 'labor',
  'humours?|rumours?|odours?|vapours?|armours?|harbours?|endeavours?': 'humor, rumor, …',
  'licences?': 'license',
  'defences?|offences?|pretences?': 'defense, offense, pretense',
  'practis(?:e|ed|es|ing)': 'practice',
  'centres?|centred|centring': 'center',
  'metres?|litres?|fibres?|theatres?|calibres?': 'meter, liter, …',
  'catalogues?|catalogued|cataloguing': 'catalog',
  'analogues?': 'analog',
  'greys?|greyed|greying|greyish': 'gray',
  'analys(?:e|ed|ing)|paralys(?:e|ed|es|ing)': 'analyze',
  'judgements?': 'judgment',
  'acknowledgements?': 'acknowledgment',
  ageing: 'aging',
  'artefacts?': 'artifact',
  'cheques?': 'check',
  'tyres?': 'tire',
  'programmes?': 'program',
  'manoeuvres?': 'maneuver',
  'sceptic(?:al|s)?': 'skeptic',
  whilst: 'while',
  amongst: 'among',
  learnt: 'learned',
  spelt: 'spelled',
  'focuss(?:ed|es|ing)': 'focused',
  'benefitt(?:ed|ing)': 'benefited',
  'enrolments?|fulfilment|instalments?': 'enrollment, fulfillment, installment',
  '(?:skil|wil)ful(?:ly)?': 'skillful, willful',
  jewellery: 'jewelry',
  orientated: 'oriented',
  'fortnights?': 'two weeks',
  // The doubled l before -ed/-ing/-er where American English keeps one.
  '(?:cancel|travel|model|label|signal|level|fuel|channel|total|marshal|tunnel|funnel|counsel|dial|pedal|portal|quarrel)l(?:ed|ing|ers?)':
    'a single l (canceled, modeling)'
};

// -ise → -ize, by stem: each matches as -ise, -ised, -ises, -ising, -iser(s),
// -isation(s) and -isable.
const ISE_STEMS = `
  apologis authoris canonicalis capitalis categoris centralis characteris criticis customis deserialis
  digitis finalis formalis generalis globalis initialis italicis localis materialis maximis memoris
  minimis modernis normalis optimis organis parameteris personalis prioritis randomis realis recognis
  regularis reinitialis reorganis sanitis serialis specialis stabilis standardis summaris symbolis
  synchronis tokenis unrecognis utilis virtualis visualis`
  .trim()
  .split(/\s+/);

function buildRules() {
  const rule = (source, american) => ({ re: new RegExp(`\\b(?:${source})\\b`, 'giu'), american });
  const rules = Object.entries(BRITISH).map(([source, american]) => rule(source, american));
  for (const stem of ISE_STEMS) rules.push(rule(`${stem}(?:e|ed|es|ing|ers?|ations?|able)`, `${stem.slice(0, -1)}z…`));
  // `emphasis` is a noun as it stands; only the verb is British.
  rules.push(rule('emphasis(?:e|ed|es|ing)', 'emphasize'));
  return rules;
}

const RULES = buildRules();

/** Findings in one file's text, as `{ line, col, message }`, in reading order. */
export function checkText(text) {
  const findings = [];
  const lines = text.split(/\r?\n/);
  lines.forEach((line, i) => {
    if (MARKER_END.test(line) || MARKER_LINE.test(lines[i - 1] ?? '')) return;
    const at = (m, message) => findings.push({ line: i + 1, col: m.index + 1, message });
    for (const m of line.matchAll(NON_LATIN_LETTER)) at(m, `non-Latin letter "${m[0]}"`);
    for (const { re, american } of RULES)
      for (const m of line.matchAll(re)) at(m, `British spelling "${m[0]}": use ${american}`);
  });
  return findings.sort((a, b) => a.line - b.line || a.col - b.col);
}

/** `path` relative to the root with forward slashes, however it was spelled. */
export function repoPath(path) {
  const slashed = path.replaceAll('\\', '/');
  return relative(ROOT, isAbsolute(slashed) ? slashed : resolve(slashed)).replaceAll('\\', '/');
}

const isChecked = path =>
  !SKIP_FILES.has(path) && !SKIP_PATHS.some(re => re.test(path)) && !BINARY_TYPES.has(fileType(path));

/** False for a symbolic link or a directory: neither is a file's text to check. */
function isPlainEntry(path) {
  const stat = lstatSync(resolve(ROOT, path), { throwIfNoEntry: false });
  return !stat || !(stat.isSymbolicLink() || stat.isDirectory());
}

const utf8 = new TextDecoder('utf-8', { fatal: true });

/** The file's text, or null when it holds a NUL or is not valid UTF-8. */
function readText(path) {
  const bytes = readFileSync(resolve(ROOT, path));
  if (bytes.includes(0)) return null;
  try {
    return utf8.decode(bytes);
  } catch {
    return null;
  }
}

/** Checks the given files and returns each finding as `path:line:col message`, the path from the root. */
export function checkFiles(paths) {
  const out = [];
  for (const path of paths.map(repoPath).filter(path => isChecked(path) && isPlainEntry(path))) {
    const text = readText(path);
    if (text === null) {
      // Binary under a type not listed above, unless its type says text.
      if (TEXT_TYPES.has(fileType(path))) out.push(`${path}:1:1 not UTF-8 text`);
      continue;
    }
    for (const f of checkText(text)) out.push(`${path}:${f.line}:${f.col} ${f.message}`);
  }
  return out;
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? '').href) {
  const args = process.argv.slice(2);
  const paths = args.length
    ? args
    : execFileSync('git', ['ls-files', '-z'], { cwd: ROOT, encoding: 'utf8' })
        .split('\0')
        .filter(Boolean)
        .map(path => resolve(ROOT, path));
  const findings = checkFiles(paths);
  for (const line of findings) console.log(line);
  if (findings.length) {
    console.error(`\n${findings.length} finding(s). A line that must stay as it is carries "${MARKER}".`);
    process.exit(1);
  }
}
