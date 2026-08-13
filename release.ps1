# Cuts a GitHub release: builds the signed installer, writes the updater
# manifest and uploads both. The version comes from src-tauri/tauri.conf.json,
# so bumping that (and package.json alongside it) is the only edit a release
# needs.
#
#   ./release.ps1                 # release the current version
#   ./release.ps1 -Notes "..."    # with release notes of your own
#   ./release.ps1 -DryRun         # build and compose, upload nothing
param(
    [string]$Notes,
    [switch]$DryRun
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
./build-windows.ps1 -Installer

$release = 'src-tauri/target/x86_64-pc-windows-msvc/release'
$portable = Join-Path $release 'luna.exe'
$setup = Get-ChildItem (Join-Path $release 'bundle/nsis') -Filter '*-setup.exe' | Select-Object -First 1
$sig = "$($setup.FullName).sig"
foreach ($f in @($portable, $setup.FullName, $sig)) {
    if (-not (Test-Path $f)) { throw "missing build artifact: $f" }
}

# What the updater plugin fetches on startup; the signature is verified against
# the public key baked into tauri.conf.json.
$manifest = [ordered]@{
    version   = $version
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

# The portable exe is renamed on the way up: luna.exe alone says nothing about
# which version someone downloaded six months ago.
$portableAsset = Join-Path $release "luna-$version-portable.exe"
Copy-Item $portable $portableAsset -Force

$notesBody = if ($Notes) { $Notes } else { "Luna $version" }
gh release create $tag $portableAsset $setup.FullName $sig $manifestPath `
    --title "Luna $version" --notes $notesBody --target master
Write-Host "Released $tag" -ForegroundColor Green
