# Releasing Luna

Releases are built, signed and attested by GitHub Actions
([`.github/workflows/release.yml`](../.github/workflows/release.yml)), not on the
maintainer's machine. The maintainer bumps the version, pushes a tag, approves the
signing job and publishes the draft the workflow leaves.

## How a release flows

1. **Bump.** `./scripts/release.ps1 -Bump 1.2.3` writes the version into the four
   files that carry it: `package.json`, `src-tauri/tauri.conf.json`,
   `src-tauri/Cargo.toml` and `src-tauri/Cargo.lock`. Write the version's section in
   `CHANGELOG.md` (`## [1.2.3] - <date>`): it is the release notes, and its opening
   paragraph, before the first `###` subsection, is what the update chip shows in
   installed copies. Commit (`Luna 1.2.3`) on a branch of its own. `master` is
   protected (see [One-time setup](#one-time-setup)), so the bump goes in through a
   pull request like any other change: push the branch, open the PR, and merge it once
   CI has passed.
2. **Tag.** On `master` as merged (`git switch master; git pull --ff-only`),
   `./scripts/release.ps1` checks that the tree is clean, HEAD is on `origin/master`,
   the four versions agree, the release notes render from `CHANGELOG.md` (the version's
   section is there, and neither it nor the section of any version since the last
   published release is empty), and the tag is free here and on `origin`. It then makes the annotated tag
   `v1.2.3` (message `Luna 1.2.3`) and prints `git push origin v1.2.3`. `-DryRun`
   checks without tagging.
3. **The workflow** runs on the tag push:

   ```
   verify ─┬─ build ── sign (signing key, `release` environment) ─┬─ attest ── publish (draft)
           └─ sbom ───────────────────────────────────────────────┘
   ```

   - **verify**: the versions agree with the tag; the tag is annotated and points at a
     commit on `master`; the release notes render from `CHANGELOG.md` against the latest
     published release, looked up once here for the whole run (a version behind it is
     refused: releases go forward only); CI (`ci.yml`) has
     passed on that very commit as a push. A CI run still going is waited for (up to 45
     minutes), so the tag can go out with its commit.
   - **build** builds the NSIS installer with `scripts/build-windows.ps1` in the same
     Docker image as a local build, and writes `latest.json` without a signature
     (`scripts/release-bundle.ps1`). It holds no secret and names no environment:
     `tauri build` runs every dependency's build scripts, proc-macros and frontend
     tooling, any of which could read a key in reach. It hands the next job the
     SHA-256 of each file as a job output.
   - **sign** waits for a reviewer's approval of the `release` environment, the one
     approval of a release, and only then gets the key. It checks out only the files
     the signing reads (the release scripts, `updater-signature.mjs`, the Windows
     Dockerfile for its pinned Node image, `pnpm-lock.yaml` and `tauri.conf.json`),
     checks the build's files against the build's checksums, and runs
     `scripts/release-sign.ps1`: it fetches the Tauri CLI's Linux build
     (`@tauri-apps/cli-linux-x64-gnu`) from the npm registry, holds it to the
     integrity `pnpm-lock.yaml` pins, the bytes the build itself ran, and runs its
     `signer sign` in the pinned Node image with no network and a read-only root. The
     signature is checked against `plugins.updater.pubkey` as installed copies check it
     (the version included) before `latest.json` takes it, so a wrong key stops the
     release here. Nothing of the build's runs in this job.
   - **sbom**: CycloneDX SBOMs of the Rust crates in the Windows build and of the npm
     packages in the frontend (`scripts/sbom.ps1`), each checked to be a file the
     attest step takes (`bomFormat`, `serialNumber` and `specVersion` all set), so a
     bad SBOM stops the run here rather than after the signed build; and the third-party
     notices (`scripts/third-party-notices.ps1`), the same file the installer carries.
   - **attest**: `SHA256SUMS` over every asset, a build provenance attestation of each,
     and the two SBOMs attested against the installer.
   - **publish** creates a **draft** release with the installer, `.sig`, `latest.json`,
     the SBOMs, the third-party notices and `SHA256SUMS`. Its notes are made from `CHANGELOG.md`
     (`scripts/release-notes.ps1`): the version's section as written; under a heading
     each, the sections of every version newer than the latest *published* release, so a
     version that was tagged but never published ships its changes here; a link to the
     compare view from that release and to the CHANGELOG; and a Verify section with the
     commands under [Verifying a release](#verifying-a-release) and the `SHA256SUMS`.
4. **Publish.** Look the draft over and publish it. Installed copies read `latest.json`
   from the latest *published* release (`plugins.updater.endpoints` in
   `tauri.conf.json`), so nothing reaches them before this.

Every job holds only the permissions it uses: `contents: read` throughout, plus
`actions: read` for verify, `id-token: write` and `attestations: write` for attest, and
`contents: write` for publish alone. Only sign holds the signing key, and only attest
the OIDC token; the build, which runs the dependencies' code, holds neither. The
workflow does not run on branch pushes or pull requests.

What the separation does not cover: a dependency that runs in the build can still put
what it likes into the installer, which sign then signs and attest attests as built
here. It keeps the key from that code, so a compromised build is one bad release, found
and replaced, rather than a key that signs updates for every installed copy from then
on.

**Dry runs.** Actions → Release → Run workflow, with `dry-run` checked (the default),
runs everything short of the attestations and the release; the assets are kept as the
run's `release-v<version>` artifact. Run on a tag, it signs as the release would (and
waits for the approval); run on a branch, the sign job passes the installer on unsigned,
with an empty signature in `latest.json`, as the key is only handed to `v*` tags. Unchecking `dry-run` on a tag releases that tag, e.g. after a run that
failed once the tag was out. A manual run uses the workflow and scripts as the chosen
ref has them: a fix to those reaches a tag only through a commit that carries it, so a
tag whose run failed on the workflow itself is released as a new version (or re-tagged
on the fixed commit, which the tag ruleset lets only the maintainer do).

**Without the workflow.** `./scripts/release.ps1 -Local` cuts the release the way it
was done before the workflow: it builds on this machine, signs with the key that
`-UpdaterKey` or `LUNA_UPDATER_KEY` names, and uploads a published release with `gh`
(no SBOMs or attestations; the notes from `CHANGELOG.md` as above, without the Verify
section). The signing is separated as in the workflow. The build runs in Docker on an
export of the commit (`git archive` into `src-tauri/target/release-source`, with no
`.git`), never on this checkout, so the code it runs cannot change the scripts that sign
afterwards; the commit the export holds is written beside it, for `-SkipBuild` to check.
Nothing the build made is run on this machine: `release-bundle.ps1` takes the core
fingerprint from the `core-fingerprint.txt` build.rs leaves, reads the installer and that
file only as plain files (a link anywhere below the export is refused), and the key is
mounted only into `release-sign.ps1`'s container. The export is deleted from inside a
container, where `rm -rf` cannot follow a junction out of it. A `-Local` build starts
from a cold target folder, so it takes as long as a first build. `-Local -DryRun`
builds, signs and composes without uploading; `-Local -SkipBuild` signs and uploads what
the last build of the same commit left.

What remains on this path: the Docker volumes the builds share (`luna-node-modules`, the
cargo registry, the xwin and Tauri caches) are written by every build, a development
build's included, so code that ran in one build can plant something the next build
runs. That reaches the next installer, which then gets signed, but not the key, which
no build container mounts. The installer itself is, as in the workflow, whatever the
build made. The workflow, whose runners start empty, has no such carry-over.

## One-time setup

1. **The `release` environment** (Settings → Environments → New environment):
   - *Required reviewers*: the maintainer. The sign job waits for this approval before
     the key is handed over; the build before it needs none, as it gets no secret.
   - *Deployment branches and tags*: selected only, with one tag rule, `v*`. No branch
     may use the environment.
   - *Environment secrets*:
     - `TAURI_SIGNING_PRIVATE_KEY`: the contents of the updater's private key file, as
       they are.
     - `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`: its password. The current key has none;
       leave the secret out and an empty password is used.

   Keep both out of the repository secrets, which reach every job of every workflow.
   - *Environment variables*: `UPDATER_TRUSTED_PUBKEY`, always defined: `embedded`, except
     for the bridge release of a key rotation (see
     [the signing key](#the-signing-key-rotation-and-recovery)). The sign step reads it
     through `vars`, which falls back to a *repository* variable of the same name when the
     environment has none, and repository variables any collaborator can create, while
     an environment's only the repository owner can (GitHub's "Store information in
     variables"). The job's token cannot list variables, so it cannot tell where a value
     came from; instead the environment's value, always there, takes precedence over any
     repository one, and the sign step stops on an empty value, which means the
     environment lost its. What is left: if the environment's variable is deleted while a
     collaborator has defined a repository one, that value is used. A wrong key still
     fails closed (the signature does not check out), so the worst it does is stop a
     release.
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
  public key in `tauri.conf.json`, still signed with the **old** key. The sign step
  checks a signature against the public key the release embeds, which for N is the new
  one, so for N give it the old one installed copies hold: set the `release`
  environment's variable `UPDATER_TRUSTED_PUBKEY` to the old `plugins.updater.pubkey`
  (or pass `-TrustedPubkey`, or set `LUNA_UPDATER_TRUSTED_PUBKEY`, for `-Local`), and
  set it back to `embedded` once N is out. Copy it from the `tauri.conf.json` of the last published
  release, never from the build. Once installed copies have taken N, replace the
  environment secrets with the new key; N+1 onward is signed with it. A copy that skipped N cannot verify N+1: its update fails on the
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
`SHA256SUMS`. The `.sig` is the updater's signature: base64 of a minisign signature file
(Ed25519 over the installer's BLAKE2b-512, with a trusted comment naming the file and the
version), which Luna checks itself against its built-in public key; `latest.json`
carries the same text. `node scripts/updater-signature.mjs verify <setup> <setup>.sig
<pubkey> 1.2.3` makes the same check, with the pubkey from `tauri.conf.json`.
