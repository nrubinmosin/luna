#Requires -Version 7.4
# Signs a release's installer for the updater and puts the signature into its
# latest.json, apart from the build. The build runs every dependency's build
# scripts and proc-macros, and any of them could read a key it was handed; so
# the build signs nothing (build-windows.ps1), and the key only ever reaches
# the system's OpenSSL 3 and the release scripts here: no dependency's code, no
# build, no container. OpenSSL opens the key (scrypt), signs (Ed25519, RFC
# 8032's deterministic signatures) and verifies; the scripts read the key file,
# compose the trusted comment as the installer is signed, and hold each
# signature to the one RFC 8032 makes for the key, so that none can carry a
# nonce chosen to leak the key. The key and its password never go on a command
# line, and no message carries anything of them. The signature is the one
# Tauri's signer made up to 0.19, so every installed copy takes it, and its
# trusted comment also names the core fingerprint, which 0.20's updater takes
# from there. The release workflow's sign job (.github/workflows/release.yml)
# and `./scripts/release.ps1 -Local` both sign here; the steps are
# release-lib.ps1's Invoke-ReleaseSign.
#
#   ./scripts/release-sign.ps1 -Dir <bundle> -Version 1.2.3 -UpdaterKey <path>   the key file
#   ./scripts/release-sign.ps1 -Dir <bundle> -Version 1.2.3                      the key in TAURI_SIGNING_PRIVATE_KEY
#   ... -TrustedPubkey <base64>                       a key rotation's bridge release (docs/releasing.md)
#   ./scripts/release-sign.ps1 -Dir <copy> -Version 1.2.3 -Throwaway             a dry run's: a key made for the run
#
# <bundle> is what release-bundle.ps1 made: the installer for -Version and a
# latest.json without a signature. -UpdaterKey defaults to LUNA_UPDATER_KEY,
# the key as `tauri signer generate` wrote it; its password is
# TAURI_SIGNING_PRIVATE_KEY_PASSWORD, empty when unset (the key kept on the
# maintainer's machine has none). The key has to be the one the signature is
# checked against, as installed copies will check it: the public key in
# src-tauri/updater.json, or, for the release that brings a new key in, the
# old one installed copies hold, which -TrustedPubkey (or
# LUNA_UPDATER_TRUSTED_PUBKEY) gives, as updater.json had it; `embedded`, or
# nothing, means the embedded one. A key installed copies would refuse stops
# the release here, before anything is signed. -RequireTrustedPubkey, which
# the workflow passes, refuses an empty value (Resolve-TrustedPubkey says why).
# The OpenSSL is /usr/bin/openssl, or on Windows the one Git for Windows
# carries (release-lib.ps1 Get-OpenSsl).
param(
    [Parameter(Mandatory)][string]$Dir,
    [Parameter(Mandatory)][string]$Version,
    [string]$UpdaterKey = $env:LUNA_UPDATER_KEY,
    [string]$TrustedPubkey = $env:LUNA_UPDATER_TRUSTED_PUBKEY,
    [switch]$RequireTrustedPubkey,
    [switch]$Throwaway
)
. "$PSScriptRoot/lib.ps1"
. "$PSScriptRoot/release-lib.ps1"

if ($Throwaway) {
    Invoke-ReleaseSign -Dir $Dir -Version $Version -Throwaway
} else {
    $trusted = Resolve-TrustedPubkey $TrustedPubkey -Required:$RequireTrustedPubkey
    Invoke-ReleaseSign -Dir $Dir -Version $Version -UpdaterKey $UpdaterKey -TrustedPubkey $trusted
}
