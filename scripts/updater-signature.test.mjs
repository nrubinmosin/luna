// Tests for updater-signature.mjs, on a signature the pinned Tauri CLI
// (2.12.3, `tauri signer sign --app-version 1.2.3`) made over the five bytes
// "TAURI" named data.bin, with a throwaway key made for this test and then
// discarded. tauri-plugin-updater's own check (minisign-verify 0.3.0) accepts
// the same signature for 1.2.3 and refuses it for 1.2.4 or other bytes.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

import { parseSignature, verifyUpdaterSignature } from './updater-signature.mjs';

const DATA = Buffer.from('TAURI');
const PUBKEY =
  'dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEEzRjM5MjE0RjE1ODFGNDgKUldSSUgxanhGSkx6bzhpdHRVZk9MOUppcWtycE5yYUQ1MGhzNys4YjdDQ25iVWZZdU1NcnFoT2wK';
const SIG =
  'dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVSSUgxanhGSkx6bzZ5RDhMc0tyeFVRb1M5MXBLL2pGNUlNRzdQQkFiNWl2YkM0WFY3MC9aMFFaSXRjbVVUVC9aa1V0aFM3bGVqek83REhCZi9HbEFuaTFJUzlYN2xrQ3c4PQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk0MTI2OTY1CWZpbGU6ZGF0YS5iaW4JdmVyc2lvbjoxLjIuMwp0dmtOZjlmWGJpcEFqSWpFUUp3SkJVQnQ3VFZobnpCMERnRS83WVUvYitXa2Zqb29WUFpEcVNIQ0tlY2lJeGpkL2Flbk1xUDhsRFMzLzdUREJzTnhBUT09Cg==';

/** SIG with one of its four lines replaced, base64 again. */
function withLine(index, replace) {
  const lines = Buffer.from(SIG, 'base64').toString('utf8').split('\n');
  lines[index] = replace(lines[index]);
  return Buffer.from(lines.join('\n')).toString('base64');
}

test('a signature the Tauri CLI made verifies for its file and version', () => {
  assert.equal(
    verifyUpdaterSignature(DATA, SIG, PUBKEY, '1.2.3', 'data.bin'),
    'timestamp:1794126965\tfile:data.bin\tversion:1.2.3'
  );
  assert.equal(parseSignature(SIG).prehashed, true);
});

test('other bytes, another version or another file name are refused', () => {
  assert.throws(() => verifyUpdaterSignature(Buffer.from('TAURJ'), SIG, PUBKEY, '1.2.3'), /does not match the file/);
  assert.throws(() => verifyUpdaterSignature(DATA, SIG, PUBKEY, '1.2.4'), /signed for version 1\.2\.3, not 1\.2\.4/);
  assert.throws(() => verifyUpdaterSignature(DATA, SIG, PUBKEY, '1.2.3', 'other.exe'), /signed as data\.bin/);
});

test('a trusted comment changed after signing is refused', () => {
  const forged = withLine(2, line => line.replace('version:1.2.3', 'version:9.9.9'));
  assert.throws(() => verifyUpdaterSignature(DATA, forged, PUBKEY, '9.9.9'), /trusted comment is not the one signed/);
});

test('a signature by another key is refused by its key id', () => {
  const conf = JSON.parse(readFileSync(new URL('../src-tauri/tauri.conf.json', import.meta.url), 'utf8'));
  assert.throws(
    () => verifyUpdaterSignature(DATA, SIG, conf.plugins.updater.pubkey, '1.2.3'),
    /signed by key A3F39214F1581F48, not by the updater's 2B43C1B12AD28622/
  );
});

test('what is not a signature is refused before any check', () => {
  assert.throws(() => verifyUpdaterSignature(DATA, '', PUBKEY, '1.2.3'), /not base64/);
  assert.throws(
    () => verifyUpdaterSignature(DATA, 'bm90IGEgc2lnbmF0dXJl', PUBKEY, '1.2.3'),
    /not base64|not a minisign/
  );
  const truncated = withLine(1, line => line.slice(0, 40));
  assert.throws(() => verifyUpdaterSignature(DATA, truncated, PUBKEY, '1.2.3'), /not a minisign signature/);
  const uncommented = withLine(2, () => 'comment: x');
  assert.throws(() => verifyUpdaterSignature(DATA, uncommented, PUBKEY, '1.2.3'), /no trusted comment/);
});
