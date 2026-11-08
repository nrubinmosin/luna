#Requires -Version 7.4
# Signs a release's installer for the updater and puts the signature into its
# latest.json, apart from the build. `tauri build` runs every dependency's
# build scripts, proc-macros and frontend tooling, and any of them could read a
# key it was handed; so the build signs nothing (build-windows.ps1), and the
# key only ever reaches one container: the pinned Node image, no network, a
# read-only root, and nothing in it but the installer and the Tauri CLI's
# native module, checked against the integrity pnpm-lock.yaml pins for it, the
# same bytes the build ran. The release workflow's sign job
# (.github/workflows/release.yml) and `./scripts/release.ps1 -Local` both sign
# here; the steps are release-lib.ps1's Invoke-ReleaseSign.
#
#   ./scripts/release-sign.ps1 -Dir <bundle> -Version 1.2.3 -UpdaterKey <path>   the key file
#   ./scripts/release-sign.ps1 -Dir <bundle> -Version 1.2.3                      the key in TAURI_SIGNING_PRIVATE_KEY
#   ... -TrustedPubkey <base64>                       a key rotation's bridge release (docs/releasing.md)
#
# <bundle> is what release-bundle.ps1 made: the installer for -Version and a
# latest.json without a signature. -UpdaterKey defaults to LUNA_UPDATER_KEY;
# TAURI_SIGNING_PRIVATE_KEY_PASSWORD is passed on when set, and an empty
# password is used otherwise (the key kept on the maintainer's machine has
# none). The signature is checked as installed copies will check it before
# latest.json takes it: against the public key in tauri.conf.json, or, for
# the release that brings a new key in, against the old one installed copies
# hold, which -TrustedPubkey (or LUNA_UPDATER_TRUSTED_PUBKEY) gives, as
# tauri.conf.json had it; `embedded`, or nothing, means the embedded one. A
# key installed copies would refuse stops the release here.
# -RequireTrustedPubkey, which the workflow passes, refuses an empty value
# (Resolve-TrustedPubkey says why).
param(
    [Parameter(Mandatory)][string]$Dir,
    [Parameter(Mandatory)][string]$Version,
    [string]$UpdaterKey = $env:LUNA_UPDATER_KEY,
    [string]$TrustedPubkey = $env:LUNA_UPDATER_TRUSTED_PUBKEY,
    [switch]$RequireTrustedPubkey
)
. "$PSScriptRoot/lib.ps1"
. "$PSScriptRoot/release-lib.ps1"

$trusted = Resolve-TrustedPubkey $TrustedPubkey -Required:$RequireTrustedPubkey
Invoke-ReleaseSign -Dir $Dir -Version $Version -UpdaterKey $UpdaterKey -TrustedPubkey $trusted
