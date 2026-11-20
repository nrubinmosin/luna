#Requires -Version 7.4
# Signs a release's installer for the updater and puts the signature into its
# latest.json, apart from the build. The build runs every dependency's build
# scripts and proc-macros, and any of them could read a key it was handed; so
# the build signs nothing (build-windows.ps1), and the key only ever reaches
# one container: the pinned Rust image, no network, a read-only root, the
# installer read-only, an empty folder for the .sig, and Luna's signer
# (src-tauri/crates/updater-signer, `luna-updater-signer`), a binary built
# beforehand in a container of its own, where its dependencies' build code ran
# with no key in reach. What the signer prints stays in its container. The
# signature is the one Tauri's signer made up to 0.19, so every installed copy
# takes it, and its trusted comment also names the core fingerprint, which
# 0.20's updater takes from there. Nothing the signer says of it is taken: on
# this host its form is held to that, field for field, and OpenSSL verifies it
# under the updater's key. The release workflow's sign job
# (.github/workflows/release.yml) and `./scripts/release.ps1 -Local` both sign
# here; the steps are release-lib.ps1's Invoke-ReleaseSign.
#
#   ./scripts/release-sign.ps1 -Dir <bundle> -Version 1.2.3 -UpdaterKey <path>   the key file
#   ./scripts/release-sign.ps1 -Dir <bundle> -Version 1.2.3                      the key in TAURI_SIGNING_PRIVATE_KEY
#   ... -TrustedPubkey <base64>                       a key rotation's bridge release (docs/releasing.md)
#   ... -Signer <path>                                the signer the workflow's signer job built
#   ./scripts/release-sign.ps1 -Dir <copy> -Version 1.2.3 -Throwaway             a dry run's: a key made for the run
#
# <bundle> is what release-bundle.ps1 made: the installer for -Version and a
# latest.json without a signature. -UpdaterKey defaults to LUNA_UPDATER_KEY,
# the key as `tauri signer generate` wrote it; TAURI_SIGNING_PRIVATE_KEY_PASSWORD
# is passed on when set, and an empty password is used otherwise (the key kept
# on the maintainer's machine has none). Without -Signer the signer is built
# first from an export of this checkout's HEAD (release-lib.ps1
# Build-UpdaterSigner). The signature is checked as installed copies will check
# it before latest.json takes it: against the public key in
# src-tauri/updater.json, or, for the release that brings a new key in, against
# the old one installed copies hold, which -TrustedPubkey (or
# LUNA_UPDATER_TRUSTED_PUBKEY) gives, as updater.json had it; `embedded`, or
# nothing, means the embedded one. A key installed copies would refuse stops
# the release here. -RequireTrustedPubkey, which the workflow passes, refuses
# an empty value (Resolve-TrustedPubkey says why). Checking takes OpenSSL 3
# (release-lib.ps1 Get-OpenSsl).
param(
    [Parameter(Mandatory)][string]$Dir,
    [Parameter(Mandatory)][string]$Version,
    [string]$UpdaterKey = $env:LUNA_UPDATER_KEY,
    [string]$TrustedPubkey = $env:LUNA_UPDATER_TRUSTED_PUBKEY,
    [string]$Signer,
    [switch]$RequireTrustedPubkey,
    [switch]$Throwaway
)
. "$PSScriptRoot/lib.ps1"
. "$PSScriptRoot/release-lib.ps1"

if ($Throwaway) {
    Invoke-ReleaseSign -Dir $Dir -Version $Version -Signer $Signer -Throwaway
} else {
    $trusted = Resolve-TrustedPubkey $TrustedPubkey -Required:$RequireTrustedPubkey
    Invoke-ReleaseSign -Dir $Dir -Version $Version -Signer $Signer -UpdaterKey $UpdaterKey -TrustedPubkey $trusted
}
