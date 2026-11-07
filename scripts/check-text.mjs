// Keeps the repository in US English. Every tracked text file is read line by
// line, and two things are reported as `path:line:col message`, exiting 1 on any:
//
// - a letter outside the Latin script (Cyrillic, Greek, CJK and the like;
//   Latin letters with diacritics, digits, punctuation and emoji pass);
// - a British spelling from the lists below, with the American one to use.
//
// Some text has to stay as it is: a test fixture that exercises non-Latin
// input, or another tool's literal output that happens to be British. Such a
// line carries the marker `lang-check: allow` in a comment, at its end or on
// a line of its own just above it, and is skipped. A trailing marker covers
// its own line only, so the prose that follows it is still checked.
//
//   node scripts/check-text.mjs            check the files git tracks
//   node scripts/check-text.mjs FILE...    check just these files

import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { extname } from 'node:path';
import { pathToFileURL } from 'node:url';

export const MARKER = 'lang-check: allow';
// The marker alone in a comment (`// …`, `# …`, `<!-- … -->`), covering the line below.
const MARKER_LINE = /^\s*(?:\/\/|#|<!--|\/\*)\s*lang-check: allow\s*(?:-->|\*\/)?\s*$/;

// Not text anyone writes (lockfiles, generated schemas), and this checker and
// its tests, which spell out the very words they look for.
const SKIP_PATHS = [/(^|\/)pnpm-lock\.yaml$/, /(^|\/)Cargo\.lock$/, /^src-tauri\/gen\//, /^scripts\/check-text\./];
const BINARY_EXTENSIONS = new Set(
  '.png .ico .icns .jpg .jpeg .gif .webp .bmp .pdf .woff .woff2 .ttf .otf .zip .gz .exe .dll .wasm'.split(' ')
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
  'labours?|laboured': 'labor',
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
    if (line.includes(MARKER) || MARKER_LINE.test(lines[i - 1] ?? '')) return;
    const at = (m, message) => findings.push({ line: i + 1, col: m.index + 1, message });
    for (const m of line.matchAll(NON_LATIN_LETTER)) at(m, `non-Latin letter "${m[0]}"`);
    for (const { re, american } of RULES)
      for (const m of line.matchAll(re)) at(m, `British spelling "${m[0]}": use ${american}`);
  });
  return findings.sort((a, b) => a.line - b.line || a.col - b.col);
}

const isChecked = path => !SKIP_PATHS.some(re => re.test(path)) && !BINARY_EXTENSIONS.has(extname(path).toLowerCase());

/** Checks the given files and returns each finding as `path:line:col message`. */
export function checkFiles(paths) {
  const out = [];
  for (const path of paths.filter(isChecked)) {
    const bytes = readFileSync(path);
    if (bytes.includes(0)) continue; // binary under an extension not listed above
    for (const f of checkText(bytes.toString('utf8'))) out.push(`${path}:${f.line}:${f.col} ${f.message}`);
  }
  return out;
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? '').href) {
  const args = process.argv.slice(2);
  const paths = args.length
    ? args
    : execFileSync('git', ['ls-files', '-z'], { encoding: 'utf8' }).split('\0').filter(Boolean);
  const findings = checkFiles(paths);
  for (const line of findings) console.log(line);
  if (findings.length) {
    console.error(`\n${findings.length} finding(s). A line that must stay as it is carries "${MARKER}".`);
    process.exit(1);
  }
}
