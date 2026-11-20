# Releasing Luna

Releases are built, signed and attested by GitHub Actions
([`.github/workflows/release.yml`](../.github/workflows/release.yml)), not on the
maintainer's machine. The maintainer bumps the version, pushes a tag, approves the
signing job and publishes the draft the workflow leaves.

## How a release flows

1. **Bump.** `./scripts/release.ps1 -Bump 1.2.3` writes the version into the files
   that carry it: `src-tauri/Cargo.toml` and `src-tauri/Cargo.lock`. Write the
   version's section in `CHANGELOG.md` (`## [1.2.3] - <date>`): it is the release notes, and its opening
   paragraph, before the first `###` subsection, is what the update chip shows in
   installed copies. Commit (`Luna 1.2.3`) on a branch of its own. `master` is
   protected (see [One-time setup](#one-time-setup)), so the bump goes in through a
   pull request like any other change: push the branch, open the PR, and merge it once
   CI has passed.
2. **Tag.** On `master` as merged (`git switch master; git pull --ff-only`),
   `./scripts/release.ps1` checks that the tree is clean, HEAD is on `origin/master`,
   the versions agree, the release notes render from `CHANGELOG.md` (the version's
   section is there, and neither it nor the section of any version since the last
   published release is empty), and the tag is free here and on `origin`. It then makes the annotated tag
   `v1.2.3` (message `Luna 1.2.3`) and prints `git push origin v1.2.3`. `-DryRun`
   checks without tagging.
3. **The workflow** runs on the tag push:

   ```
   verify ─┬─ build ── sign (signing key, `release` environment) ─┬─ attest ── publish (draft)
           └─ sbom ─────────────────────────────────────────────────┘
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
     the build runs every dependency's build scripts and proc-macros, any of which
     could read a key in reach. It hands the next job the SHA-256 of each file as a
     job output.
   - **sign** waits for a reviewer's approval of the `release` environment, the one
     approval of a release, and only then gets the key. It checks out only the files
     the signing reads (the release scripts and `src-tauri/updater.json`), checks the
     build's files against their job's checksums, and runs `scripts/release-sign.ps1`,
     which signs with the runner image's own OpenSSL 3 (Ubuntu's `/usr/bin/openssl`)
     and the release scripts' PowerShell, and nothing else: no dependency's code, no
     build, no container. OpenSSL opens the key (scrypt, as minisign encrypted it) and
     signs (Ed25519 as RFC 8032 has it, deterministic); the scripts check the key's
     checksum and that it is the key the signature is checked against, write the
     trusted comment as they sign, and hold every signature to the one RFC 8032 makes
     for that key and message, so that none can carry a nonce chosen to give the key
     away. The key and its password are never on a command line, nor in the
     environment of anything the scripts start: OpenSSL is the only process they run
     while the key is held, and takes the password on its input, as the UTF-8 bytes the
     Tauri CLI and minisign derive the key from, so a password beyond ASCII opens it on
     Windows as on Linux. No message says anything of them. The signature is in Tauri's form, which 0.19 reads, with the
     core fingerprint added to its trusted comment. Its form is then held to exactly
     that, every field of the trusted comment to this installer, version, core and the
     time of signing; OpenSSL verifies both its signatures under the public key in
     `src-tauri/updater.json` (BLAKE2b-512 of the installer, Ed25519); and the bundle
     must be file for file what it was. Only then does `latest.json` take it, so a
     wrong key stops the release here, before anything is signed. Nothing of the
     build's runs in this job.
   - **sbom**: a CycloneDX SBOM of the Rust crates in the Windows build
     (`scripts/sbom.ps1`), checked to be a file the attest step takes (`bomFormat`,
     `serialNumber` and `specVersion` all set), so a bad SBOM stops the run here rather
     than after the signed build; and the third-party notices
     (`scripts/third-party-notices.ps1`), the same file the installer carries.
   - **attest**: `SHA256SUMS` over every asset, a build provenance attestation of each,
     and the SBOM attested against the installer.
   - **publish** creates a **draft** release with the installer, `.sig`, `latest.json`,
     the SBOM, the third-party notices and `SHA256SUMS`. Its notes are made from `CHANGELOG.md`
     (`scripts/release-notes.ps1`): the version's section as written; under a heading
     each, the sections of every version newer than the latest *published* release, so a
     version that was tagged but never published ships its changes here; a link to the
     compare view from that release and to the CHANGELOG; and a Verify section with the
     commands under [Verifying a release](#verifying-a-release) and the `SHA256SUMS`.
4. **Publish.** Look the draft over and publish it. Installed copies read `latest.json`
   from the latest *published* release (`endpoint` in `src-tauri/updater.json`), so
   nothing reaches them before this.

Every job holds only the permissions it uses: `contents: read` throughout, plus
`actions: read` for verify, `id-token: write` and `attestations: write` for attest, and
`contents: write` for publish alone. Only sign holds the signing key, and only attest
the OIDC token; the build, which runs the dependencies' code, holds neither. The
workflow does not run on branch pushes or pull requests.

What the separation does not cover: a dependency that runs in the build can still put
what it likes into the installer, which sign then signs and attest attests as built
here. It keeps the key from that code, so a compromised build is one bad release, found
and replaced, rather than a key that signs updates for every installed copy from then
on. The signing itself runs no package of anyone's: the key is handled by the
runner image's OpenSSL and the release scripts alone, so no dependency can read it
or choose what a signature carries. That matters beyond reading it: a valid Ed25519
signature whose nonce someone knows gives them the key, and a signer that picked its
nonces (minisign hedges them with fresh noise, as the Tauri CLI's signer did up to
0.19) could leak the key in signatures every verifier takes. Here every signature is
RFC 8032's deterministic one, checked against what the RFC derives from the key and
the message. What the job does trust with the key is the runner image, its pwsh and
its OpenSSL, as it trusts it with the secret in its environment.

**Dry runs.** Actions → Release → Run workflow, with `dry-run` checked (the default),
runs everything short of the attestations and the release; the assets are kept as the
run's `release-v<version>` artifact. Run on a tag, it signs as the release would (and
waits for the approval); run on a branch, the sign job signs a copy of the bundle with a
key made for the run with OpenSSL, kept in memory and thrown away
(`release-sign.ps1 -Throwaway`), so the signing itself is tried, and passes the
installer on unsigned, with an empty signature in
`latest.json`, as the key is only handed to `v*` tags. Unchecking `dry-run` on a tag releases that tag, e.g. after a run that
failed once the tag was out. A manual run uses the workflow and scripts as the chosen
ref has them: a fix to those reaches a tag only through a commit that carries it, so a
tag whose run failed on the workflow itself is released as a new version (or re-tagged
on the fixed commit, which the tag ruleset lets only the maintainer do).

**Without the workflow.** `./scripts/release.ps1 -Local` cuts the release the way it
was done before the workflow: it builds on this machine, signs with the key that
`-UpdaterKey` or `LUNA_UPDATER_KEY` names, and uploads a published release with `gh`
(no SBOMs or attestations; the notes from `CHANGELOG.md` as above, without the Verify
section). The signing is the workflow's: `release-sign.ps1` signs on this machine with
the OpenSSL 3 Git for Windows carries (`/usr/bin/openssl` elsewhere) and the release
scripts, nothing else holding the key, and checks the signature with OpenSSL. That
OpenSSL is found under the folder Git for Windows' installer registered in
`HKLM\SOFTWARE\GitForWindows` (`InstallPath`), which only an administrator can change,
never through `PATH` or by running `git`: Git for Windows has to be installed for all
users, and the folder it was installed in is trusted as it is, so keep it where only
administrators can write (`C:\Program Files\Git`, its default). The build
runs in Docker on an
export of the commit (`git archive` into `src-tauri/target/release-source`, with no
`.git`), never on this checkout, so the code it runs cannot change the scripts that sign
afterwards; the commit the export holds is written beside it, for `-SkipBuild` to check.
Nothing the build made is run on this machine: `release-bundle.ps1` takes the core
fingerprint from the `core-fingerprint.txt` build.rs leaves, reads the installer and that
file only as plain files (a link anywhere below the export is refused), and the key is
read by `release-sign.ps1` alone, never by a container. The export is deleted from inside a
container, where `rm -rf` cannot follow a junction out of it. A `-Local` build starts
from a cold target folder, so it takes as long as a first build. `-Local -DryRun`
builds, signs and composes without uploading; `-Local -SkipBuild` signs and uploads what
the last build of the same commit left.

What remains on this path: the Docker volumes the builds share (the cargo registry and
the xwin cache) are written by every build, a development build's included, so code that ran in one build can plant something the next build
runs. That reaches the next installer, which then gets signed, but not the key, which
no container sees. The installer itself is, as in the workflow, whatever the
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
       leave the secret out and an empty password is used. A password may be any
       Unicode text but a line break or another control character, up to 1023 bytes as
       UTF-8 (what OpenSSL reads of one on its input); the signing refuses the rest.
       The key's scrypt parameters have to be the ones every key the Tauri CLI or
       minisign makes has (N = 2^15, r = 8, p = 1), or at least within N 2^20 and p 64:
       a key made with others is refused on purpose.

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
3. **Protect `master`**: require pull requests and the CI checks (`scripts`,
   `rust-linux`, `windows-build`, `windows-tests`), and block force pushes. The release
   checks CI on its commit itself, but only a protected `master` makes "on `master`"
   mean reviewed.
4. **Actions settings**: the default `GITHUB_TOKEN` permission set to read; the workflow
   asks for more job by job.
5. **Attestations** need a public repository (or GitHub Enterprise Cloud for a private
   one).

## The signing key: rotation and recovery

The updater installs an update only if its `.sig` verifies against the public key built
into the running Luna. Luna 0.20 reads `pubkey` from `src-tauri/updater.json`.
Luna 0.19 pins the same key in its updater plugin. A new key takes effect
once the installed copy takes a release carrying it.

Keep an offline copy of the private key and its password (a password manager): GitHub
never shows a secret again.

- **Planned rotation.** Generate a new key with OpenSSL through the release scripts
  (`. ./scripts/lib.ps1; . ./scripts/release-lib.ps1; New-UpdaterKey <file>`): the
  secret key at `<file>`, in the Tauri CLI's form with an empty password, and its
  public half at `<file>.pub`. The key file is readable by you alone from the moment it
  is made: mode 600 on Linux and macOS; on Windows a protected DACL, nothing inherited
  from its folder, that lets in you, SYSTEM and Administrators only, as Luna's own
  private files have. Release N with the **new** public key in
  `src-tauri/updater.json`, still signed with the **old** key. The sign step
  checks a signature against the public key the release embeds, which for N is the new
  one, so for N give it the old one installed copies hold: set the `release`
  environment's variable `UPDATER_TRUSTED_PUBKEY` to the old `pubkey`
  (or pass `-TrustedPubkey`, or set `LUNA_UPDATER_TRUSTED_PUBKEY`, for `-Local`), and
  set it back to `embedded` once N is out. Copy it from the `updater.json` of the last
  published release, never from the build. Once installed copies have taken N, replace the
  environment secrets with the new key; N+1 onward is signed with it. A copy that skipped N cannot verify N+1: its update fails on the
  signature, and its user installs N+1 by hand from the release page, over the existing
  install; Luna keeps its data in the install folder, so nothing is lost.
- **Lost key.** No installed copy can be updated automatically again. Put a new key's
  public half in `src-tauri/updater.json` and release; every user installs that release by hand
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

# The SBOM attested against the installer.
gh attestation verify Luna_1.2.3_x64-setup.exe --repo nrubinmosin/luna \
  --predicate-type https://cyclonedx.org/bom

# Every asset against the checksums.
sha256sum -c SHA256SUMS
```

In PowerShell, compare `(Get-FileHash Luna_1.2.3_x64-setup.exe).Hash` with its line in
`SHA256SUMS`. The `.sig` is the updater's signature: base64 of a minisign signature file
(Ed25519 over the installer's BLAKE2b-512, with a trusted comment naming the file, the
version and the core fingerprint), which Luna checks itself against its built-in public
key; `latest.json` carries the same text. From a checkout, OpenSSL 3 (on PATH, or Git
for Windows' own) makes the same check, with no build, and
holds the `.sig` to its form and `latest.json` to the `.sig`:

```powershell
. ./scripts/lib.ps1; . ./scripts/release-lib.ps1
Assert-ReleaseSignature <folder> 1.2.3            # -Pubkey <old key> for a bridge release
```
