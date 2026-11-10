---
name: release
description: Cut a Luna release end to end — version bump, CHANGELOG entry, the annotated tag from scripts/release.ps1, the tag push, watching .github/workflows/release.yml to the sign job's approval, verifying the draft (installer and its .sig, latest.json, SHA256SUMS, SBOMs, third-party notices, gh attestation verify) and publishing it. Use it whenever someone asks to release, ship, tag or publish a Luna version, bump the version, check or publish a release draft, verify release assets or attestations, asks what to do about a release run waiting for approval, or why a release run failed, even if they only say "let's do 1.2.3" or "is the draft good?".
---

# Release

`docs/releasing.md` is the source of truth for how a release flows and for the
one-time GitHub setup; read it before the first step. This skill is the order
of work and where to stop for the person.

Three steps leave the machine and cannot be taken back: getting the bump onto
`master`, pushing the tag (that starts the workflow), and publishing the draft
(that reaches every installed Luna through `latest.json`). Do each only when
the person asked for exactly that. "Release 1.2.3" asks for the bump and the
tag. "Prepare" or "get ready" a release asks for the local work only: the bump
commit, the gate, the release notes drafted, and the exact commands that
remain, printed for them. `release.ps1` tags only a HEAD that is already on
`origin/master`, and the workflow only a commit CI has passed, so the tag
itself waits until the bump has landed and CI is green on it. Publishing
always needs their word after they have seen your check of the draft. When in
doubt, prepare and ask.

## 1. Before anything

- On `master`, clean tree, up to date with `origin/master`.
- `./scripts/gate.ps1` green on that commit; CI on `master` green too (the
  workflow refuses a tag whose commit CI has not passed: `gh run list
  --workflow ci.yml --branch master -L 3`).
- The live suite is recommended before a release (`docs/testing.md`; the
  `live-test` skill). Say if it was not run.
- Pick the version with the person if they did not name one: what changed since
  the last tag (`git log --oneline $(git describe --tags --abbrev=0)..`). A
  named version that skips or reuses a number against the last tag is worth one
  line of question before the bump.

## 2. Bump

```powershell
./scripts/release.ps1 -Bump 1.2.3
```

Writes the four version files and nothing else. Write the version's section in
`CHANGELOG.md`: it is the release notes (`release.ps1` refuses a missing or
empty one), and its opening paragraph is what the update chip shows. Write it
from the commits, user-facing, short. Commit as `Luna 1.2.3` (the repo's convention) and
get it onto `master` the way `master` takes changes: a pull request when it is
protected (`docs/releasing.md`, "One-time setup"), else a push. Wait for CI on
that commit before tagging.

## 3. Tag

```powershell
./scripts/release.ps1 -DryRun                   # all checks, no tag
./scripts/release.ps1                           # annotated tag v1.2.3
git push origin v1.2.3                          # starts release.yml
```

The notes come from the CHANGELOG section, not the tag. Fix whatever
`-DryRun` refuses rather than working around it.

## 4. Watch the workflow

```powershell
gh run list --workflow release.yml -L 3
gh run watch <run-id> --exit-status
```

The jobs run `verify`, then `build` and `sbom` side by side, then `sign`, then
`attest` and `publish` (`docs/releasing.md`, "How a release flows"). The one
that waits is **sign**: it alone binds the `release` environment and gets the
signing key, so a reviewer's approval of that environment is the release's
one approval. `build` holds no secret and needs none, so by the time the run
waits, the installer and an unsigned `latest.json` are already built.

A run can also look stuck in `verify`: its log repeating `CI on <sha> is
in_progress; checking again in 30 s` is `release-verify.ps1` waiting for CI on
the tagged commit (up to 45 minutes, 5 when CI never ran on it). There is
nothing to approve yet.

The approval is the person's click in GitHub (Review deployments on the run),
or `gh` run by them; tell them it is waiting rather than approving it
yourself, and that it should not wait for days: the `unsigned` artifact `sign`
downloads is kept for 7 days, after which an approved `sign` has nothing to
download and the tag needs a fresh run. Give them what the approval vouches
for:

- the run is for tag `v1.2.3` (`gh run view <id>` names the ref) and the tag's
  commit is the bump on `master`;
- `verify` and `build` passed, and build's `sums` step lists the installer and
  `latest.json` with their SHA-256: the files `sign` will hold the artifact to
  before it signs.

Once approved, `sign` checks the bundle against those sums, signs the
installer, checks the signature against `plugins.updater.pubkey` in
`src-tauri/tauri.conf.json` (as installed copies check it) and writes it into
`latest.json`; `attest` and `publish` then leave the draft. A key rotation's
bridge release, which embeds the new public key but is signed with the old
key, is checked against the old public key instead: the `release`
environment's variable `UPDATER_TRUSTED_PUBKEY` gives it for that release,
`embedded` for every other (`docs/releasing.md`, "The signing key: rotation
and recovery").

On a failure, `gh run view <id> --log-failed`, find the step, and bring a
diagnosis. In `sign` the step says what it means:

- *Check the bundle against the build's checksums*: the artifact `sign`
  downloaded is not what `build` hashed. Do not re-run past it; find what
  changed it.
- *Sign the installer*: the Tauri CLI download did not match the integrity
  `pnpm-lock.yaml` pins, or the key does not sign for the updater's public
  key (a wrong or rotated secret, or a bridge release run without
  `UPDATER_TRUSTED_PUBKEY`), or the `release` environment does not define
  `UPDATER_TRUSTED_PUBKEY` at all. Nothing was signed into `latest.json`.

A re-run on the same tag is Actions → Release → Run workflow on the tag with
`dry-run` unchecked (`docs/releasing.md`, "Dry runs").

## 5. Verify the draft

Download into a scratch folder under `test-build/` and check every item; the
commands are in `docs/releasing.md` ("Verifying a release").

```powershell
gh release view v1.2.3 --json isDraft,assets,tagName,body
gh release download v1.2.3 -D test-build/release-v1.2.3
```

- **Assets**: seven — the installer `Luna_1.2.3_x64-setup.exe`, its `.sig`,
  `latest.json`, `luna-1.2.3-cargo.cdx.json`, `luna-1.2.3-npm.cdx.json`,
  `luna-1.2.3-third-party-notices.txt` (from the `sbom` job) and
  `SHA256SUMS`. Nothing missing, nothing extra; it is still a draft.
- **SHA256SUMS**: every other asset listed and matching (`sha256sum -c
  SHA256SUMS`, or `Get-FileHash` per line).
- **Attestations**: `gh attestation verify <asset> --repo nrubinmosin/luna
  --signer-workflow nrubinmosin/luna/.github/workflows/release.yml` for each
  asset `SHA256SUMS` lists (the provenance covers those, not the sums file
  itself), and both SBOM attestations on the installer (`--predicate-type
  https://cyclonedx.org/bom`).
- **latest.json**: `version` is 1.2.3; `platforms.windows-x86_64.url` is
  `https://github.com/nrubinmosin/luna/releases/download/v1.2.3/<installer
  name>`; its `signature` equals the `.sig` file's contents, compared as
  strings by a command, not by eye; `core_fingerprint` is present; `notes` is
  the CHANGELOG section's opening paragraph.
- **Updater signature**: the `.sig` is not empty (only a dry run off a branch
  leaves it so) and verifies against the updater's public key for this
  version and file name, the check every installed Luna makes before it
  installs. There is no local Node, so run it in the pinned Node image, with
  no network:

  ```powershell
  . ./scripts/lib.ps1; . ./scripts/release-lib.ps1
  $d = (Resolve-Path test-build/release-v1.2.3).Path
  docker run --rm --network none -v "${d}:/out:ro" `
    -v "$PWD/scripts/updater-signature.mjs:/s.mjs:ro" (Get-NodeImage) `
    node /s.mjs verify /out/Luna_1.2.3_x64-setup.exe /out/Luna_1.2.3_x64-setup.exe.sig (Get-UpdaterPubkey) 1.2.3
  ```

  For a bridge release, the key is the old one `UPDATER_TRUSTED_PUBKEY` held,
  in place of `(Get-UpdaterPubkey)`.
- **Notes**: the draft's body is the CHANGELOG section, the sections of any
  version since the last published release, the compare link and the Verify
  block whose `SHA256SUMS` match the asset.
- **Plausibility**: the installer is megabytes, not kilobytes; each SBOM lists
  the project's real dependencies; the third-party notices open with
  `Third-party notices for Luna 1.2.3`, this version, and list the components
  the installer carries, since they are the same file it installs beside
  `luna.exe`. A file that hashes right can still be the wrong file.

Report each check as passed or failed with the evidence (hashes compared,
`gh attestation verify` result lines). Any failure means no publish: find the
cause (the run's logs; `scripts/release-bundle.ps1` writes `latest.json`,
`scripts/release-sign.ps1` its signature) and
propose a fix and a re-run or a new version; never edit an asset of the draft
by hand, which would break its checksums and attestations.

## 6. Publish — only on the person's word

```powershell
gh release edit v1.2.3 --draft=false
```

Then confirm `https://github.com/nrubinmosin/luna/releases/latest/download/latest.json`
serves the new version.

## Without the workflow

`./scripts/release.ps1 -Local` builds, signs and uploads a **published** release
from this machine (no SBOMs, no attestations), with the key `-UpdaterKey` or
`LUNA_UPDATER_KEY` names. The build never sees the key: it is mounted only into
`release-sign.ps1`'s container, as in the workflow. The build runs on an export
of the commit in `src-tauri/target/release-source`, not on the checkout, and
nothing it made runs on this machine; `-SkipBuild` signs only an export of the
same commit, and a `-Local` build starts from a cold target folder. What only
the workflow's fresh runners rule out are the Docker cache volumes every local
build shares (`docs/releasing.md`, "Without the workflow"). A bridge release
of a key rotation takes the old public key as `-TrustedPubkey` (or
`LUNA_UPDATER_TRUSTED_PUBKEY`). It is the fallback when the workflow cannot
run; use it only when the person chose it, and `-Local -DryRun` first.
