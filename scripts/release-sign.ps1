#Requires -Version 7.4
# Signs a release's installer for the updater and puts the signature into its
# latest.json, apart from the build. `tauri build` runs every dependency's
# build scripts, proc-macros and frontend tooling, and any of them could read a
# key it was handed; so the build signs nothing (build-windows.ps1), and the
# key only ever reaches this script's one container: the pinned Node image,
# no network, a read-only root, and nothing in it but the installer and the
# Tauri CLI's native module, checked against the integrity pnpm-lock.yaml pins
# for it, the same bytes the build ran. The release workflow's sign job
# (.github/workflows/release.yml) and `./scripts/release.ps1 -Local` both sign
# here.
#
#   ./scripts/release-sign.ps1 -Dir <bundle> -Version 1.2.3 -UpdaterKey <path>   the key file
#   ./scripts/release-sign.ps1 -Dir <bundle> -Version 1.2.3                      the key in TAURI_SIGNING_PRIVATE_KEY
#
# <bundle> is what release-bundle.ps1 made: the installer for -Version and a
# latest.json without a signature. -UpdaterKey defaults to LUNA_UPDATER_KEY;
# TAURI_SIGNING_PRIVATE_KEY_PASSWORD is passed on when set, and an empty
# password is used otherwise (the key kept on the maintainer's machine has
# none). The signature is checked against the updater's public key in
# tauri.conf.json, as installed copies will check it, before latest.json
# takes it: a key that is not the updater's stops the release here.
param(
    [Parameter(Mandatory)][string]$Dir,
    [Parameter(Mandatory)][string]$Version,
    [string]$UpdaterKey = $env:LUNA_UPDATER_KEY
)
. "$PSScriptRoot/lib.ps1"
. "$PSScriptRoot/release-lib.ps1"

Assert-VersionFormat $Version
$Dir = [IO.Path]::GetFullPath($Dir, (Get-Location).Path)
$setups = @(Get-ChildItem -LiteralPath $Dir -Filter "*_${Version}_*-setup.exe" -File -ErrorAction SilentlyContinue)
if ($setups.Count -ne 1) { throw "$Dir should hold one installer for $Version, not $($setups.Count)" }
$setup = $setups[0]
if (Test-Path -LiteralPath "$($setup.FullName).sig") { throw "$($setup.Name) is signed already" }
$manifestPath = Join-Path $Dir 'latest.json'
if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) { throw "no latest.json in $Dir" }
$pubkey = Get-UpdaterPubkey
$endpoint = Get-UpdaterEndpoint

# Passed on by name, its value from this process's environment, so it never
# shows on a command line. Left out, the CLI takes the key to have no password.
$keyArgs = @()
if ($env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD) { $keyArgs += @('-e', 'TAURI_SIGNING_PRIVATE_KEY_PASSWORD') }
if ($UpdaterKey) {
    if (-not (Test-Path -LiteralPath $UpdaterKey -PathType Leaf)) {
        throw "no updater key at $UpdaterKey (from -UpdaterKey or LUNA_UPDATER_KEY)"
    }
    # Mounted rather than passed as an env value, so it never shows in
    # `docker inspect` or the process list on the host.
    $keyPath = (Resolve-Path -LiteralPath $UpdaterKey).Path
    $keyArgs += @('-v', "${keyPath}:/keys/updater.key:ro", '-e', 'TAURI_SIGNING_PRIVATE_KEY_PATH=/keys/updater.key')
} elseif ($env:TAURI_SIGNING_PRIVATE_KEY) {
    $keyArgs += @('-e', 'TAURI_SIGNING_PRIVATE_KEY')
} else {
    throw 'signing needs the updater''s private key: pass -UpdaterKey <path>, or set LUNA_UPDATER_KEY or TAURI_SIGNING_PRIVATE_KEY'
}

# The CLI's Linux build, the package @tauri-apps/cli installs on a Linux host,
# as the build's own `pnpm install` fetched and checked it.
$cli = Get-LockedNpmPackage '@tauri-apps/cli-linux-x64-gnu'
$work = Join-Path $RepoRoot "src-tauri/target/release-signer/$([guid]::NewGuid().ToString('n'))"
New-Item -ItemType Directory -Force (Join-Path $work 'cli') | Out-Null
try {
    $tarball = Join-Path $work 'cli.tgz'
    Write-Host "Tauri CLI $($cli.Version) from $($cli.Url)"
    Invoke-WebRequest -Uri $cli.Url -OutFile $tarball -MaximumRetryCount 3 -RetryIntervalSec 5
    Assert-Sha512Integrity $tarball $cli.Integrity
    Export-TarballEntry $tarball 'package/cli.linux-x64-gnu.node' (Join-Path $work 'cli/cli.node')

    $image = Get-NodeImage
    $isolated = @(
        '--rm', '--network', 'none', '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges',
        '-v', "$(Join-Path $work 'cli'):/cli:ro",
        '-v', "$(Join-Path $PSScriptRoot 'updater-signature.mjs'):/sign/updater-signature.mjs:ro"
    )
    $file = "/out/$($setup.Name)"
    Invoke-Checked 'tauri signer sign' {
        docker run @isolated @keyArgs -v "${Dir}:/out" $image node /sign/updater-signature.mjs sign /cli/cli.node $file $Version
    }
    # Checked in a container of its own, with no key and the bundle read-only.
    # A .sig that fails is taken away again: no installed Luna would take it.
    try {
        Invoke-Checked 'the signature check' {
            docker run @isolated -v "${Dir}:/out:ro" $image node /sign/updater-signature.mjs verify $file "$file.sig" $pubkey $Version
        }
    } catch {
        Remove-Item -LiteralPath "$($setup.FullName).sig" -ErrorAction SilentlyContinue
        throw
    }

    $signature = (Get-Content -LiteralPath "$($setup.FullName).sig" -Raw).Trim()
    $manifest = Set-UpdaterManifestSignature -Manifest (Get-Content -LiteralPath $manifestPath -Raw) -Signature $signature `
        -Version $Version -SetupName $setup.Name -Endpoint $endpoint
    $manifest | Set-Content -LiteralPath $manifestPath -Encoding utf8
    Write-Host "signed $($setup.Name) for v$Version; latest.json carries the signature"
} finally {
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}
