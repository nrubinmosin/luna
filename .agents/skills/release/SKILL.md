---
name: release
description: Cut a Luna release end to end — version bump, CHANGELOG entry, the annotated tag from scripts/release.ps1, the tag push, watching .github/workflows/release.yml, verifying the draft (assets, SHA256SUMS, gh attestation verify, latest.json) and publishing it. Use it whenever someone asks to release, ship, tag or publish a Luna version, bump the version, check or publish a release draft, verify release assets or attestations, or asks why a release run failed, even if they only say "let's do 1.2.3" or "is the draft good?".
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

The **build** job waits for a reviewer to approve the `release` environment.
That is the person's click in GitHub (or `gh` by them); tell them it is
waiting rather than approving it yourself. On a failure, `gh run view <id>
--log-failed`, find the step, and bring a diagnosis; a re-run on the same tag is
Actions → Release → Run workflow on the tag with `dry-run` unchecked
(`docs/releasing.md`, "Dry runs").

## 5. Verify the draft

Download into a scratch folder under `test-build/` and check every item; the
commands are in `docs/releasing.md` ("Verifying a release").

```powershell
gh release view v1.2.3 --json isDraft,assets,tagName,body
gh release download v1.2.3 -D test-build/release-v1.2.3
```

- **Assets**: the installer `Luna_1.2.3_x64-setup.exe`, its `.sig`,
  `latest.json`, `luna-1.2.3-cargo.cdx.json`, `luna-1.2.3-npm.cdx.json`,
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
- **Notes**: the draft's body is the CHANGELOG section, the sections of any
  version since the last published release, the compare link and the Verify
  block whose `SHA256SUMS` match the asset.
- **Plausibility**: the installer is megabytes, not kilobytes; each SBOM lists
  the project's real dependencies. A file that hashes right can still be the
  wrong file.

Report each check as passed or failed with the evidence (hashes compared,
`gh attestation verify` result lines). Any failure means no publish: find the
cause (the run's logs, `scripts/release-bundle.ps1` for `latest.json`) and
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
`LUNA_UPDATER_KEY` names. It is the fallback when the workflow cannot run; use
it only when the person chose it, and `-Local -DryRun` first.
