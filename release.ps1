# Cuts a GitHub release: builds the signed installer, writes the updater
# manifest and uploads both. The version comes from src-tauri/tauri.conf.json,
# so bumping that (and package.json alongside it) is the only edit a release
# needs.
#
#   ./release.ps1                 # release the current version
#   ./release.ps1 -Notes "..."    # with release notes of your own
#   ./release.ps1 -DryRun         # build and compose, upload nothing
#   ./release.ps1 -SkipBuild      # upload what the last build of this commit
#                                 # left behind, e.g. after a failed upload
param(
    [string]$Notes,
    [switch]$DryRun,
    [switch]$SkipBuild
)
$ErrorActionPreference = 'Stop'

$conf = Get-Content src-tauri/tauri.conf.json -Raw | ConvertFrom-Json
$version = $conf.version
$tag = "v$version"
$pkg = Get-Content package.json -Raw | ConvertFrom-Json
if ($pkg.version -ne $version) {
    throw "package.json says $($pkg.version), tauri.conf.json says $version — bump both."
}

# A release has to be reproducible from a commit, and the build stamp baked into
# the binary is only meaningful if the tree it was built from is the tree that
# was pushed.
if (git status --porcelain) { throw 'Working tree is dirty — commit before releasing.' }
if (git log origin/master..HEAD --oneline) { throw 'HEAD is ahead of origin/master — push before releasing.' }
if (gh release view $tag 2>$null) { throw "$tag already exists — bump the version." }

Write-Host "Releasing $tag" -ForegroundColor Cyan
# A native command failing does not trip $ErrorActionPreference, so each one
# that matters is checked by hand: a failed build would otherwise go on to
# upload whatever the previous build left behind.
if (-not $SkipBuild) {
    ./build-windows.ps1 -Installer
    if ($LASTEXITCODE) { throw "build failed (exit $LASTEXITCODE)" }
}

$release = 'src-tauri/target/x86_64-pc-windows-msvc/release'
# The bare exe goes up no more: the updater installs the setup into the folder
# Luna was installed in, so a copy run from anywhere else updated into a
# second Luna with none of its data. It is still built, and asked for the
# fingerprint below.
$exe = Join-Path $release 'luna.exe'
# By version, not by whatever comes first: the bundle directory keeps every
# installer ever built there, and picking the first one alphabetically is how
# v0.2.0 went out carrying the 0.1.0 setup and its signature — an update that
# offered a build its own version.
$setup = Get-ChildItem (Join-Path $release 'bundle/nsis') -Filter "*_${version}_*-setup.exe" | Select-Object -First 1
if (-not $setup) { throw "no installer for $version in $release/bundle/nsis" }
$sig = "$($setup.FullName).sig"
foreach ($f in @($exe, $setup.FullName, $sig)) {
    if (-not (Test-Path $f)) { throw "missing build artifact: $f" }
}

# The fingerprint of the Rust side (build.rs). A window whose core has the same
# one can install this update by restarting only itself: the core, and every
# session in it, carries on (updates.store.ts).
$fpFile = Join-Path $release 'core-fingerprint.txt'
Remove-Item $fpFile -ErrorAction SilentlyContinue
Start-Process -FilePath $exe -ArgumentList '--core-fingerprint', "`"$fpFile`"" -Wait
if (-not (Test-Path $fpFile)) { throw 'the exe did not report its core fingerprint' }
$coreFingerprint = (Get-Content $fpFile -Raw).Trim()

# What the updater plugin fetches on startup; the signature is verified against
# the public key baked into tauri.conf.json.
$manifest = [ordered]@{
    version   = $version
    core_fingerprint = $coreFingerprint
    notes     = if ($Notes) { $Notes } else { "Luna $version" }
    pub_date  = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
    platforms = [ordered]@{
        'windows-x86_64' = [ordered]@{
            signature = (Get-Content $sig -Raw).Trim()
            url       = "https://github.com/nrubinmosin/luna/releases/download/$tag/$($setup.Name)"
        }
    }
}
$manifestPath = Join-Path $release 'latest.json'
$manifest | ConvertTo-Json -Depth 5 | Set-Content $manifestPath -Encoding utf8
Write-Host "manifest -> $manifestPath"

if ($DryRun) {
    Write-Host 'Dry run: nothing uploaded.' -ForegroundColor Yellow
    return
}

$notesBody = if ($Notes) { $Notes } else { "Luna $version" }
gh release create $tag $setup.FullName $sig $manifestPath `
    --title "Luna $version" --notes $notesBody --target master
# How v0.4.2 first went out: gh timed out on the TLS handshake, created nothing,
# and the script still said Released.
if ($LASTEXITCODE) {
    throw "gh release create failed (exit $LASTEXITCODE). If 'gh release view $tag' finds nothing, rerun with -SkipBuild; if the release is there with assets missing, finish it with 'gh release upload $tag'."
}
Write-Host "Released $tag" -ForegroundColor Green
