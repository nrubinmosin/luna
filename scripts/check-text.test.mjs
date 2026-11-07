// Tests for check-text.mjs, on fixture files under test-build/ (ignored by git).
import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { join, relative } from 'node:path';
import { after, test } from 'node:test';

import { ROOT, checkFiles, repoPath } from './check-text.mjs';

mkdirSync(join(ROOT, 'test-build'), { recursive: true });
const dir = mkdtempSync(join(ROOT, 'test-build', 'check-text-'));
const prefix = relative(ROOT, dir).replaceAll('\\', '/') + '/';
after(() => rmSync(dir, { recursive: true, force: true }));

/** The findings for one fixture file holding `content`, without its path. */
function check(name, content) {
  const path = join(dir, name);
  writeFileSync(path, content);
  return checkFiles([path]).map(line => {
    assert.ok(line.startsWith(prefix + name + ':'), line);
    return line.slice(prefix.length + name.length + 1);
  });
}

test('a Cyrillic letter is flagged where it stands', () => {
  assert.deepEqual(check('cyrillic.rs', 'let a = 1;\nlet greeting = "привет";\n'), [
    '2:17 non-Latin letter "п"',
    '2:18 non-Latin letter "р"',
    '2:19 non-Latin letter "и"',
    '2:20 non-Latin letter "в"',
    '2:21 non-Latin letter "е"',
    '2:22 non-Latin letter "т"'
  ]);
});

test('a marker comment at the end of a line or alone above it lets the line pass', () => {
  assert.deepEqual(check('marked.rs', 'let a = "привет"; // lang-check: allow\n'), []);
  assert.deepEqual(check('above.rs', '    // lang-check: allow\nlet a = "colour";\n'), []);
  assert.deepEqual(check('above.md', '<!-- lang-check: allow -->\nIn colour.\n'), []);
  assert.deepEqual(check('trailing.md', 'In colour. <!-- lang-check: allow -->\n'), []);
  assert.deepEqual(check('hash.toml', 'a = "colour" # lang-check: allow\n'), []);
  assert.deepEqual(check('block.css', '/* lang-check: allow */\na { content: "colour"; }\n'), []);
  assert.deepEqual(check('doc.ts', '/**\n * lang-check: allow\n * In colour.\n */\n'), []);
});

test('a trailing marker does not cover the line after it', () => {
  assert.deepEqual(check('next.rs', 'let a = "colour"; // lang-check: allow\n// in colour\n'), [
    '2:7 British spelling "colour": use color'
  ]);
});

test('the marker inside a string is no marker', () => {
  // One British word and six Cyrillic letters.
  assert.equal(check('string.ts', 'const x = "lang-check: allow colour привет";\n').length, 7);
  assert.deepEqual(check('string-end.ts', 'const x = "colour // lang-check: allow"\n'), [
    '1:12 British spelling "colour": use color'
  ]);
});

test('a malformed marker is no marker', () => {
  for (const line of [
    'colour // lang-check: allowed',
    'colour // lang-check: allow colour',
    'colour // lang-check allow',
    'colour //lang-check:allow',
    'colour lang-check: allow'
  ])
    assert.equal(check('malformed.rs', line + '\n').length >= 1, true, line);
  assert.equal(check('malformed-above.rs', '// lang-check: allowing\ncolour\n').length, 1);
});

test('a British spelling is flagged with the American one', () => {
  assert.deepEqual(check('colour.ts', '// The Colour of a chat.\r\n'), ['1:8 British spelling "Colour": use color']);
  assert.deepEqual(check('cancelled.ts', 'it was cancelled'), [
    '1:8 British spelling "cancelled": use a single l (canceled, modeling)'
  ]);
  assert.equal(check('labour.md', 'labour, labouring').length, 2);
});

test('-ise forms of a listed stem are flagged', () => {
  assert.deepEqual(
    check('ise.md', 'organise initialised recognising').map(f => f.split(' ').slice(0, 4).join(' ')),
    ['1:1 British spelling "organise":', '1:10 British spelling "initialised":', '1:22 British spelling "recognising":']
  );
});

test('American spellings and words that only contain a British one pass', () => {
  assert.deepEqual(check('color.ts', '// The color of a chat, canceled; analyses; emphasis; colour_scheme.\n'), []);
});

test('words ending in -ise that are not British pass', () => {
  const words =
    'precise promise exercise otherwise noise raise advise compromise enterprise expertise surprise revise ' +
    'supervise concise disguise';
  assert.deepEqual(check('ise-words.md', words + '\n'), []);
});

test('Latin letters with diacritics and emoji pass', () => {
  assert.deepEqual(check('latin.md', 'Café, naïve, Ünïcode, Łódź 🎨\n'), []);
});

test('a text type that is not UTF-8 is a finding', () => {
  assert.deepEqual(check('nul.rs', Buffer.from('let a = 1;\0\n')), ['1:1 not UTF-8 text']);
  assert.deepEqual(check('latin1.md', Buffer.from([0x63, 0x61, 0x66, 0xe9, 0x0a])), ['1:1 not UTF-8 text']);
});

test('a file of another type that is not UTF-8 is taken for binary', () => {
  assert.deepEqual(check('blob.dat', 'colour\0привет'), []);
  assert.deepEqual(check('blob2.dat', Buffer.from([0xff, 0xfe, 0x00])), []);
});

test('a path is taken relative to the root however it is spelled', () => {
  process.chdir(ROOT);
  const expected = 'src-tauri/gen/x.json';
  for (const spelling of [
    join(ROOT, 'src-tauri', 'gen', 'x.json'),
    ROOT.replaceAll('\\', '/') + 'src-tauri/gen/x.json',
    ROOT + 'src-tauri\\gen\\x.json',
    './src-tauri/gen/x.json',
    'src-tauri\\gen\\x.json'
  ])
    assert.equal(repoPath(spelling), expected, spelling);
});

test('the checker and its tests are skipped by their exact paths from the root', () => {
  const own = ['check-text.mjs', 'check-text.test.mjs'].map(name => join(ROOT, 'scripts', name));
  assert.deepEqual(checkFiles([...own, ...own.map(path => path.replaceAll('/', '\\'))]), []);
  // The same names anywhere else are checked like any file.
  mkdirSync(join(dir, 'scripts'), { recursive: true });
  assert.equal(check('scripts/check-text.mjs', 'colour\n').length, 1);
  assert.equal(check('check-text.mjs.md', 'colour\n').length, 1);
});
