// Tests for check-text.mjs, on fixture files in a temporary folder.
import assert from 'node:assert/strict';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { after, test } from 'node:test';

import { checkFiles } from './check-text.mjs';

const dir = mkdtempSync(join(tmpdir(), 'check-text-'));
after(() => rmSync(dir, { recursive: true, force: true }));

/** The findings for one fixture file holding `text`, without its path. */
function check(name, text) {
  const path = join(dir, name);
  writeFileSync(path, text);
  return checkFiles([path]).map(line => line.slice(path.length + 1));
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

test('a marked line passes, at its end or on a line of its own above', () => {
  assert.deepEqual(check('marked.rs', 'let a = "привет"; // lang-check: allow\n'), []);
  assert.deepEqual(check('above.rs', '// lang-check: allow\nlet a = "colour";\n'), []);
  assert.deepEqual(check('above.md', '<!-- lang-check: allow -->\nIn colour.\n'), []);
});

test('a trailing marker does not cover the line after it', () => {
  assert.deepEqual(check('next.rs', 'let a = "colour"; // lang-check: allow\n// in colour\n'), [
    '2:7 British spelling "colour": use color'
  ]);
});

test('a British spelling is flagged with the American one', () => {
  assert.deepEqual(check('colour.ts', '// The Colour of a chat.\r\n'), ['1:8 British spelling "Colour": use color']);
  assert.deepEqual(check('cancelled.ts', 'it was cancelled'), [
    '1:8 British spelling "cancelled": use a single l (canceled, modeling)'
  ]);
});

test('American spellings and words that only contain a British one pass', () => {
  assert.deepEqual(check('color.ts', '// The color of a chat, canceled; analyses; emphasis; colour_scheme.\n'), []);
});

test('Latin letters with diacritics and emoji pass', () => {
  assert.deepEqual(check('latin.md', 'Café, naïve, Ünïcode, Łódź 🎨\n'), []);
});

test('a binary file is skipped', () => {
  assert.deepEqual(check('blob.dat', 'colour\0привет'), []);
});
