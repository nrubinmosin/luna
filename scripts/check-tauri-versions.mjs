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
//
//   node scripts/check-tauri-versions.mjs

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

/** The repository's root. */
export const ROOT = fileURLToPath(new URL('..', import.meta.url));

/** Every version of each package Cargo.lock locks, by crate name. */
export function cargoVersions(lockText) {
  const out = new Map();
  for (const block of lockText
    .replaceAll('\r\n', '\n')
    .split(/^\[\[package\]\]$/m)
    .slice(1)) {
    const name = /^name = "([^"]+)"$/m.exec(block)?.[1];
    const version = /^version = "([^"]+)"$/m.exec(block)?.[1];
    if (!name || !version) continue;
    out.set(name, [...(out.get(name) ?? []), version]);
  }
  return out;
}

/**
 * The version pnpm installs for each direct dependency of the root project,
 * from pnpm-lock.yaml's `importers: .:` section: what `pnpm list`, which
 * tauri-cli asks, reports.
 */
export function npmVersions(lockText) {
  const out = new Map();
  const lines = lockText.replaceAll('\r\n', '\n').split('\n');
  const start = lines.indexOf('importers:');
  if (start < 0) return out;
  let inRoot = false;
  let name = null;
  for (const line of lines.slice(start + 1)) {
    if (/^\S/.test(line)) break; // the next top-level key
    if (/^ {2}\S/.test(line)) {
      inRoot = line === '  .:';
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

/** Each pair whose locked versions tauri build would refuse, as a line saying so. */
export function mismatches(cargoLockText, pnpmLockText) {
  const npm = npmVersions(pnpmLockText);
  const out = [];
  for (const [crate, versions] of cargoVersions(cargoLockText)) {
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
  const found = mismatches(
    readFileSync(join(ROOT, 'src-tauri', 'Cargo.lock'), 'utf8'),
    readFileSync(join(ROOT, 'pnpm-lock.yaml'), 'utf8')
  );
  for (const m of found) console.log(m);
  if (found.length) {
    console.error(
      `\n${found.length} mismatch(es): tauri build refuses these. Move the npm package to the crate's major.minor (pnpm add <package>@^x.y.0), or the crate to the package's.`
    );
    process.exit(1);
  }
}
