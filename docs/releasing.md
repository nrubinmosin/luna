# Releasing Luna

Releases are built, signed and attested by GitHub Actions
([`.github/workflows/release.yml`](../.github/workflows/release.yml)), not on the
maintainer's machine. The maintainer bumps the version, pushes a tag, approves the
signing step and publishes the draft the workflow leaves.

## How a release flows

1. **Bump.** `./scripts/release.ps1 -Bump 1.2.3` writes the version into the four
   files that carry it: `package.json`, `src-tauri/tauri.conf.json`,
   `src-tauri/Cargo.toml` and `src-tauri/Cargo.lock`. Add the `CHANGELOG.md` entry if
   there is one, commit (`Luna 1.2.3`) and push `master`.
2. **Tag.** `./scripts/release.ps1` (or `-Notes "..."`) checks that the tree is clean,
   HEAD is on `origin/master`, the four versions agree, `CHANGELOG.md` has a heading for
   the version (a warning when there is no CHANGELOG), and the tag is free here and on
   `origin`. It then makes the annotated tag `v1.2.3`, whose message becomes the release
   notes, and prints `git push origin v1.2.3`. `-DryRun` checks without tagging.
3. **The workflow** runs on the tag push:

   ```
   verify ─┬─ build (signing key, `release` environment) ─┬─ attest ── publish (draft)
           └─ sbom ───────────────────────────────────────┘
   ```

   - **verify**: the versions agree with the tag; the tag is annotated and points at a
     commit on `master`; CI (`ci.yml`) has passed on that very commit as a push. A CI run
     still going is waited for (up to 45 minutes), so the tag can go out with its commit.
   - **build** waits for a reviewer's approval of the `release` environment, then builds
     the NSIS installer and its updater signature with `scripts/build-windows.ps1` in the
     same Docker image as a local build, and writes `latest.json`
     (`scripts/release-bundle.ps1`).
   - **sbom**: CycloneDX SBOMs of the Rust crates in the Windows build and of the npm
     packages in the frontend (`scripts/sbom.ps1`).
   - **attest**: `SHA256SUMS` over every asset, a build provenance attestation of each,
     and the two SBOMs attested against the installer.
   - **publish** creates a **draft** release with the installer, `.sig`, `latest.json`,
     the SBOMs and `SHA256SUMS`.
4. **Publish.** Look the draft over and publish it. Installed copies read `latest.json`
   from the latest *published* release (`plugins.updater.endpoints` in
   `tauri.conf.json`), so nothing reaches them before this.

Every job holds only the permissions it uses: `contents: read` throughout, plus
`actions: read` for verify, `id-token: write` and `attestations: write` for attest, and
`contents: write` for publish alone. The workflow does not run on branch pushes or pull
requests.

**Dry runs.** Actions → Release → Run workflow, with `dry-run` checked (the default),
runs everything short of the attestations and the release; the assets are kept as the
run's `release-v<version>` artifact. Run on a tag, it signs as the release would (and
waits for the approval); run on a branch, it builds unsigned, as the key is only handed
to `v*` tags. Unchecking `dry-run` on a tag releases that tag, e.g. after a run that
failed once the tag was out.

**Without the workflow.** `./scripts/release.ps1 -Local` cuts the release the way it
was done before the workflow: it builds and signs on this machine with the key that
`-UpdaterKey` or `LUNA_UPDATER_KEY` names, and uploads a published release with `gh`
(no SBOMs or attestations). `-Local -DryRun` builds and composes without uploading;
`-Local -SkipBuild` uploads what the last build left.

## One-time setup

1. **The `release` environment** (Settings → Environments → New environment):
   - *Required reviewers*: the maintainer. The build job waits for this approval before
     the key is handed over.
   - *Deployment branches and tags*: selected only, with one tag rule, `v*`. No branch
     may use the environment.
   - *Environment secrets*:
     - `TAURI_SIGNING_PRIVATE_KEY`: the contents of the updater's private key file, as
       they are.
     - `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`: its password. The current key has none;
       leave the secret out and an empty password is used.

   Keep both out of the repository secrets, which reach every job of every workflow.
2. **A tag ruleset** (Settings → Rules → Rulesets → New tag ruleset) for `v*`: restrict
   creations, updates and deletions, with the maintainer as the only bypass. Only they
   can then push a release tag, or move one.
3. **Protect `master`**: require pull requests and the CI checks (`frontend`,
   `rust-linux`, `windows-build`, `windows-tests`), and block force pushes. The release
   checks CI on its commit itself, but only a protected `master` makes "on `master`"
   mean reviewed.
4. **Actions settings**: the default `GITHUB_TOKEN` permission set to read; the workflow
   asks for more job by job.
5. **Attestations** need a public repository (or GitHub Enterprise Cloud for a private
   one).

## The signing key: rotation and recovery

The updater installs an update only if its `.sig` verifies against the public key built
into the running Luna, `plugins.updater.pubkey` in `tauri.conf.json`. Changing that key
reaches only copies that have already installed a release carrying the new one.

Keep an offline copy of the private key and its password (a password manager): GitHub
never shows a secret again.

- **Planned rotation.** Generate a new key (`pnpm tauri signer generate -w <file>`, in
  the Windows build image or anywhere the Tauri CLI runs). Release N with the **new**
  public key in `tauri.conf.json`, still signed with the **old** key. Once installed
  copies have taken N, replace the environment secrets with the new key; N+1 onward is
  signed with it. A copy that skipped N cannot verify N+1: its update fails on the
  signature, and its user installs N+1 by hand from the release page, over the existing
  install; Luna keeps its data in the install folder, so nothing is lost.
- **Lost key.** No installed copy can be updated automatically again. Put a new key's
  public half in `tauri.conf.json` and release; every user installs that release by hand
  once (say so in its notes and in the README), and updates carry on from there.
- **Leaked key.** Whoever holds it can sign an update that every installed copy accepts,
  though it still has to be served from this repository's latest release. The updater
  has no revocation: rotate at once as planned (release N, signed with the old key,
  carrying the new public key), check the repository's releases and access, and treat
  copies that never take N as trusting the leaked key.

## Verifying a release

With the assets of a release downloaded into one folder:

```sh
# Built by this repository's release workflow, from the tagged commit.
gh attestation verify Luna_1.2.3_x64-setup.exe --repo nrubinmosin/luna \
  --signer-workflow nrubinmosin/luna/.github/workflows/release.yml

# The SBOMs attested against the installer.
gh attestation verify Luna_1.2.3_x64-setup.exe --repo nrubinmosin/luna \
  --predicate-type https://cyclonedx.org/bom

# Every asset against the checksums.
sha256sum -c SHA256SUMS
```

In PowerShell, compare `(Get-FileHash Luna_1.2.3_x64-setup.exe).Hash` with its line in
`SHA256SUMS`. The `.sig` is the updater's minisign signature, which Luna checks itself
against its built-in public key; `latest.json` carries the same signature.
