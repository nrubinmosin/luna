// The updater signature of a release installer, made and checked outside the
// build (scripts/release-sign.ps1 runs this in the pinned Node image, with no
// network and nothing of the build's in reach).
//
//   node updater-signature.mjs sign <cli.node> <file> <version>
//   node updater-signature.mjs verify <file> <file.sig> <pubkey> <version>
//
// sign runs `tauri signer sign` from the Tauri CLI's native module, the one
// pnpm-lock.yaml pins (release-sign.ps1 checks the download against it), with
// the key from TAURI_SIGNING_PRIVATE_KEY or TAURI_SIGNING_PRIVATE_KEY_PATH and
// TAURI_SIGNING_PRIVATE_KEY_PASSWORD, as `tauri build` would have signed it:
// the version goes into the signature's trusted comment. It writes
// <file>.sig: base64 of the minisign signature file.
//
// verify checks a .sig the way tauri-plugin-updater does before it installs an
// update (updater.rs verify_signature, on minisign-verify): the public key and
// the signature base64-decoded, the key id matched, the Ed25519 signature over
// the file's BLAKE2b-512 (or over the file itself, minisign's legacy form,
// which the plugin allows), the global signature over that signature and the
// trusted comment, and the version the comment names against the release's.
// It asks a little more than the plugin, which takes a signature with no
// version: every signature made here carries one, so one without it is not ours.

import { createHash, createPublicKey, verify } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { basename } from 'node:path';
import { pathToFileURL } from 'node:url';

/** The text a base64 value encodes; throws on anything that is not base64. */
function fromBase64(value, what) {
  const text = value.trim();
  if (!/^[A-Za-z0-9+/]+={0,2}$/.test(text)) throw new Error(`${what} is not base64`);
  return Buffer.from(text, 'base64');
}

/** A minisign public key, as tauri.conf.json holds it: base64 of the .pub file. */
export function parsePublicKey(base64) {
  const lines = fromBase64(base64, 'the public key').toString('utf8').split(/\r?\n/);
  const bin = fromBase64(lines[1] ?? '', 'the public key line');
  if (bin.length !== 42 || bin[0] !== 0x45 || (bin[1] !== 0x64 && bin[1] !== 0x44)) {
    throw new Error('not a minisign Ed25519 public key');
  }
  return { keyId: bin.subarray(2, 10), key: bin.subarray(10, 42) };
}

/** A Tauri updater signature: base64 of a minisign signature file's four lines. */
export function parseSignature(base64) {
  const lines = fromBase64(base64, 'the signature').toString('utf8').split(/\r?\n/);
  const bin = fromBase64(lines[1] ?? '', 'the signature line');
  const global = fromBase64(lines[3] ?? '', 'the global signature line');
  if (bin.length !== 74 || global.length !== 64) throw new Error('not a minisign signature');
  if (!(lines[2] ?? '').startsWith('trusted comment: ')) throw new Error('the signature has no trusted comment');
  const algorithm = bin.subarray(0, 2).toString('latin1');
  if (algorithm !== 'ED' && algorithm !== 'Ed') throw new Error(`unsupported signature algorithm ${algorithm}`);
  return {
    prehashed: algorithm === 'ED',
    keyId: bin.subarray(2, 10),
    signature: bin.subarray(10, 74),
    trustedComment: lines[2].slice('trusted comment: '.length),
    global
  };
}

/** A key id as minisign prints it ("minisign public key: 2B43..."): its bytes reversed, in hex. */
function keyIdText(keyId) {
  return Buffer.from(keyId).reverse().toString('hex').toUpperCase();
}

function ed25519Verify(message, key, signature) {
  const publicKey = createPublicKey({
    key: { kty: 'OKP', crv: 'Ed25519', x: key.toString('base64url') },
    format: 'jwk'
  });
  return verify(null, message, publicKey, signature);
}

/**
 * Throws unless `signature` (a .sig's text) signs `data` under `pubkey` (as
 * tauri.conf.json holds it) for `version`, the file being named `fileName`.
 * Returns the trusted comment.
 */
export function verifyUpdaterSignature(data, signature, pubkey, version, fileName) {
  const key = parsePublicKey(pubkey);
  const sig = parseSignature(signature);
  if (!key.keyId.equals(sig.keyId)) {
    throw new Error(`signed by key ${keyIdText(sig.keyId)}, not by the updater's ${keyIdText(key.keyId)}`);
  }
  const message = sig.prehashed ? createHash('blake2b512').update(data).digest() : data;
  if (!ed25519Verify(message, key.key, sig.signature)) throw new Error('the signature does not match the file');
  const comment = Buffer.from(sig.trustedComment, 'utf8');
  if (!ed25519Verify(Buffer.concat([sig.signature, comment]), key.key, sig.global)) {
    throw new Error('the trusted comment is not the one signed');
  }
  // Read only now: before the global signature checked out, it was anyone's text.
  const fields = sig.trustedComment.split('\t');
  const signed = fields.find(field => field.startsWith('version:'))?.slice('version:'.length);
  if (signed !== version) throw new Error(`signed for version ${signed ?? '(none)'}, not ${version}`);
  const file = fields.find(field => field.startsWith('file:'))?.slice('file:'.length);
  if (fileName !== undefined && file !== fileName) throw new Error(`signed as ${file ?? '(no file)'}, not ${fileName}`);
  return sig.trustedComment;
}

/** Runs `tauri signer sign` from the CLI's native module at `cliPath`. */
function sign(cliPath, file, version) {
  const cli = createRequire(import.meta.url)(cliPath);
  return new Promise((resolve, reject) => {
    cli.run(['signer', 'sign', '--app-version', version, file], 'tauri', (error, done) => {
      if (error) reject(error);
      else if (!done) reject(new Error('tauri signer sign did not finish'));
      else resolve();
    });
  });
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [command, ...args] = process.argv.slice(2);
  try {
    if (command === 'sign' && args.length === 3) {
      await sign(...args);
    } else if (command === 'verify' && args.length === 4) {
      const [file, sigFile, pubkey, version] = args;
      const comment = verifyUpdaterSignature(
        readFileSync(file),
        readFileSync(sigFile, 'utf8'),
        pubkey,
        version,
        basename(file)
      );
      console.log(`${basename(sigFile)} verifies against the updater's public key: ${comment.replaceAll('\t', ' ')}`);
    } else {
      throw new Error(
        'usage: updater-signature.mjs sign <cli.node> <file> <version> | verify <file> <file.sig> <pubkey> <version>'
      );
    }
  } catch (error) {
    console.error(`updater-signature: ${error.message}`);
    process.exit(1);
  }
}
