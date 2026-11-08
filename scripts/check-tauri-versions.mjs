// Keeps each Tauri crate and its npm package on one release line. `tauri build`
// refuses to build when they part (tauri-cli's `check_mismatched_packages`,
// src/info/plugins.rs): the crate `tauri` against `@tauri-apps/api`, and each
// `tauri-plugin-<name>` against `@tauri-apps/plugin-<name>`, by the versions
// actually locked, which must agree in major and minor; the patch may differ.
// The gate builds neither the app nor a bundle, and Dependabot moves the cargo
// and npm sides in separate PRs, so this applies the same rule to the lockfiles
// and fails the gate where tauri build would. No network, no install: what is
// compared is what src-tauri/Cargo.lock and pnpm-lock.yaml say.
//
// A pair counts only when both sides are there, as in tauri-cli: a plugin with
// no npm half (single-instance, window-state) has nothing to be compared with.
// Every `tauri-plugin-<name>` is paired, not only the plugins tauri-cli's list
// names: an official plugin missing from that list still has to match, and the
// stricter rule costs nothing here.
//
// A lockfile this does not read is a failure, not a pass: Cargo.lock formats
// 3 and 4 and pnpm's lockfileVersion 9.0 are read, anything else is refused,
// and so is a pair of lockfiles without `tauri` and `@tauri-apps/api`, which
// every Tauri app locks. A parser gone out of date then stops the gate rather
// than finding nothing to compare.
//
//   node scripts/check-tauri-versions.mjs

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

/** The repository's root. */
export const ROOT = fileURLToPath(new URL('..', import.meta.url));

/** The Cargo.lock formats read here (its top-level `version = N`). */
const CARGO_FORMATS = ['3', '4'];
/** The pnpm-lock.yaml format read here. */
const PNPM_FORMAT = '9.0';

/** Every version of each package Cargo.lock locks, by crate name. Throws on a lockfile it does not read. */
export function cargoVersions(lockText) {
  const [head, ...blocks] = lockText.replaceAll('\r\n', '\n').split(/^\[\[package\]\]$/m);
  const format = /^version = (\S+)$/m.exec(head)?.[1];
  if (format === undefined) throw new Error('Cargo.lock: no format version (version = N) before its packages');
  if (!CARGO_FORMATS.includes(format)) {
    throw new Error(`Cargo.lock: format version ${format}, and only ${CARGO_FORMATS.join(' and ')} are read here`);
  }
  if (!blocks.length) throw new Error('Cargo.lock: no [[package]] in it');
  const out = new Map();
  for (const block of blocks) {
    const name = /^name = "([^"]+)"$/m.exec(block)?.[1];
    const version = /^version = "([^"]+)"$/m.exec(block)?.[1];
    if (!name || !version)
      throw new Error('Cargo.lock: a [[package]] without a name and version in the form read here');
    out.set(name, [...(out.get(name) ?? []), version]);
  }
  return out;
}

/**
 * The version pnpm installs for each direct dependency of the root project,
 * from pnpm-lock.yaml's `importers: .:` section: what `pnpm list`, which
 * tauri-cli asks, reports. Throws on a lockfile it does not read.
 */
export function npmVersions(lockText) {
  const out = new Map();
  const lines = lockText.replaceAll('\r\n', '\n').split('\n');
  const format = lines.map(l => /^lockfileVersion: '?([^'\s]+)'?$/.exec(l)?.[1]).find(Boolean);
  if (format === undefined) throw new Error('pnpm-lock.yaml: no lockfileVersion');
  if (format !== PNPM_FORMAT) {
    throw new Error(`pnpm-lock.yaml: lockfileVersion '${format}', and only '${PNPM_FORMAT}' is read here`);
  }
  const start = lines.indexOf('importers:');
  if (start < 0) throw new Error("pnpm-lock.yaml: no root project (importers: '.')");
  let inRoot = false;
  let sawRoot = false;
  let name = null;
  for (const line of lines.slice(start + 1)) {
    if (/^\S/.test(line)) break; // the next top-level key
    if (/^ {2}\S/.test(line)) {
      inRoot = line === '  .:';
      sawRoot ||= inRoot;
      continue;
    }
    if (!inRoot) continue;
    const dep = /^ {6}'?([^':]+)'?:$/.exec(line);
    if (dep) {
      name = dep[1];
      continue;
    }
    const version = /^ {8}version: (\S+)$/.exec(line);
    // A version may carry its peers in parentheses: 1.2.3(react@19.0.0).
    if (version && name) out.set(name, version[1].replace(/\(.*$/, ''));
  }
  if (!sawRoot) throw new Error("pnpm-lock.yaml: no root project (importers: '.')");
  return out;
}

/** The npm package that goes with a Tauri crate, or null for any other crate. */
export function npmName(crate) {
  if (crate === 'tauri') return '@tauri-apps/api';
  const plugin = /^tauri-plugin-(.+)$/.exec(crate);
  return plugin ? `@tauri-apps/plugin-${plugin[1]}` : null;
}

/** "major.minor" of a semver version. */
function line(version) {
  const m = /^(\d+)\.(\d+)\./.exec(version);
  return m ? `${m[1]}.${m[2]}` : version;
}

/**
 * Each pair whose locked versions tauri build would refuse, as a line saying
 * so. Throws on a lockfile it does not read, or one without the `tauri` pair.
 */
export function mismatches(cargoLockText, pnpmLockText) {
  const cargo = cargoVersions(cargoLockText);
  const npm = npmVersions(pnpmLockText);
  if (!cargo.has('tauri')) throw new Error('no tauri in Cargo.lock: not a Tauri app, or not read right');
  if (!npm.has('@tauri-apps/api')) {
    throw new Error("no @tauri-apps/api in pnpm-lock.yaml's root project: not a Tauri app, or not read right");
  }
  const out = [];
  for (const [crate, versions] of cargo) {
    const pkg = npmName(crate);
    const npmVersion = pkg && npm.get(pkg);
    if (!npmVersion) continue;
    for (const version of versions) {
      if (line(version) !== line(npmVersion)) {
        out.push(`${crate} ${version} (Cargo.lock) and ${pkg} ${npmVersion} (pnpm-lock.yaml) differ in major.minor`);
      }
    }
  }
  return out;
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? '').href) {
  let found;
  try {
    found = mismatches(
      readFileSync(join(ROOT, 'src-tauri', 'Cargo.lock'), 'utf8'),
      readFileSync(join(ROOT, 'pnpm-lock.yaml'), 'utf8')
    );
  } catch (e) {
    console.error(
      `${e.message}\nThe Tauri versions are not compared: bring scripts/check-tauri-versions.mjs to the lockfile's form.`
    );
    process.exit(1);
  }
  for (const m of found) console.log(m);
  if (found.length) {
    console.error(
      `\n${found.length} mismatch(es): tauri build refuses these. Move the npm package to the crate's major.minor (pnpm add <package>@^x.y.0), or the crate to the package's.`
    );
    process.exit(1);
  }
}
