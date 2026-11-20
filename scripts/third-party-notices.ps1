#Requires -Version 7.4
# The third-party notices the installer ships beside luna.exe and the release
# publishes as an asset: the license and source for each component compiled
# into the Windows exe or carried by the installer. Made in Docker using the
# Cargo lockfile, with the source mounted read-only.
#
#   ./scripts/third-party-notices.ps1 -Out <file>
#   ... -Source <tree>      of another tree than this checkout (build-windows.ps1 -Source)
#
#   - the Rust standard library, with the inventory of its own sources and the
#     crates it is built from that the pinned toolchain carries
#     (share/doc/rust/COPYRIGHT-library.html) and the license texts it names;
#   - the crates the Windows build links (cargo metadata for its target, normal
#     dependencies only: no build script, proc macro or test dependency);
#   - what the installer carries outside the crate graph (NSIS, the adapted
#     installer template, the font and the VC runtime).
#
# A component without a license text of its own takes the one kept for it in
# third-party-licenses/; one with neither fails the script (notices-lib.ps1).
param(
    [Parameter(Mandatory)][string]$Out,
    [string]$Source
)
. "$PSScriptRoot/lib.ps1"
. "$PSScriptRoot/release-lib.ps1"
. "$PSScriptRoot/notices-lib.ps1"

$version = Get-ReleaseVersion
$Out = [IO.Path]::GetFullPath($Out, (Get-Location).Path)
$Source = if ($Source) { [IO.Path]::GetFullPath($Source, (Get-Location).Path) } else { $RepoRoot }
$rust = Get-BaseImages | Where-Object { $_.File -eq 'docker/windows-build.Dockerfile' -and $_.Repo -eq 'rust' }
if (@($rust).Count -ne 1) { throw 'docker/windows-build.Dockerfile should have one FROM rust: line' }
$toolchain = $rust.Tag -replace '-.*$', ''
$supplied = Join-Path $Source 'third-party-licenses'

Initialize-HostDirs 'test-build'
# The container runs as root, and cp keeps a file's mode, so a crate
# published with 0640 files (fnv 1.0.7) came back root's own and unreadable on
# a Linux host such as the Actions runner, its folders undeletable. Each
# run hands /out back on its way out, a failure included: readable to
# all, and on a Unix host owned by whoever runs this. Not `docker run --user`:
# the cache volumes they share with the builds, and the images' own folders,
# are root's. A mount of a Windows folder keeps no Unix mode or owner, so on a
# Windows host this changes nothing.
$owner = if ($IsWindows) { @() } else { @('-e', "OUT_OWNER=$(id -u):$(id -g)") }
$handBack = 'trap ''chmod -R a+rX /out; if [ -n "${OUT_OWNER:-}" ]; then chown -R "$OUT_OWNER" /out; fi'' EXIT' + "`n"
$stage = Join-Path $RepoRoot "test-build/notices/$([guid]::NewGuid().ToString('n'))"
New-Item -ItemType Directory -Force $stage | Out-Null
try {
    Use-BuildLock {
        # The sources of every package in the target's tree, the registry's and
        # those vendored in src-tauri/vendor/ (a patched crate, under its
        # package's name), and of each the files that hold its license, under
        # <name>-<version>/ by their path in the package. RUSTUP_TOOLCHAIN
        # keeps rustup to the image's own toolchain, whose doc folder holds the
        # standard library's inventory.
        Invoke-Checked 'crate licenses' {
            docker run --rm `
              -e RUSTUP_TOOLCHAIN=$toolchain `
              @owner `
              -v "${Source}:/app:ro" `
              -v "${stage}:/out" `
              -v luna-cargo-registry:/usr/local/cargo/registry `
              -w /app/src-tauri `
              $rust.Ref `
              bash -c (ConvertTo-BashScript ($handBack + @'
set -e
cargo metadata --locked --format-version 1 --filter-platform x86_64-pc-windows-msvc > /out/metadata.json
grep -o '"manifest_path":"[^"]*"' /out/metadata.json | cut -d'"' -f4 | while read -r manifest; do
  dir=$(dirname "$manifest")
  case "$dir" in
    /usr/local/cargo/registry/*) pkg=$(basename "$dir") ;;
    /app/src-tauri/vendor/*) pkg="$(basename "$dir")-$(sed -n 's/^version = "\(.*\)"$/\1/p' "$dir/Cargo.toml" | head -n 1)" ;;
    *) continue ;;
  esac
  (cd "$dir" && find . -maxdepth 2 -type f \( -iname 'licen[cs]e*' -o -iname 'copying*' -o -iname 'notice*' -o -iname 'unlicense*' -o -ipath './licenses/*' \)) | while read -r file; do
    mkdir -p "/out/crates/$pkg/$(dirname "$file")"
    cp "$dir/$file" "/out/crates/$pkg/$file"
  done
done
mkdir -p /out/rust-doc
cp -r "$(rustc --print sysroot)/share/doc/rust/COPYRIGHT-library.html" "$(rustc --print sysroot)/share/doc/rust/licenses" /out/rust-doc/
'@))
        }
    }

    # Every crate the exe links, and the standard library under them.
    $metadata = Get-Content (Join-Path $stage 'metadata.json') -Raw | ConvertFrom-Json -Depth 64
    $crates = foreach ($package in Get-ShippedCrates $metadata) {
        $origin = "https://crates.io/crates/$($package.name)/$($package.version)"
        if ($package.repository) { $origin += " (repository: $($package.repository))" }
        [pscustomobject]@{
            Name     = $package.name
            Version  = $package.version
            License  = $package.license
            Source   = $origin
            Note     = Get-LicenseNote $package.license
            Staged   = Join-Path $stage "crates/$($package.name)-$($package.version)"
            Supplied = Join-Path $supplied "crates/$($package.name)"
        }
    }
    # The standard library: its inventory as text, then the license texts its
    # in-tree files are under, which the inventory only names.
    $html = Get-Content -LiteralPath (Join-Path $stage 'rust-doc/COPYRIGHT-library.html') -Raw
    $stdDir = Join-Path $stage 'rust-std'
    New-Item -ItemType Directory -Force (Join-Path $stdDir 'licenses') | Out-Null
    [IO.File]::WriteAllText((Join-Path $stdDir 'COPYRIGHT-library.txt'), (ConvertFrom-RustCopyright $html))
    foreach ($id in Get-RustInTreeLicenses $html) {
        $text = Join-Path $stage "rust-doc/licenses/$id.txt"
        if (-not (Test-Path -LiteralPath $text)) { throw "the toolchain's COPYRIGHT-library names $id, and its licenses folder has no $id.txt" }
        Copy-Item -LiteralPath $text -Destination (Join-Path $stdDir 'licenses')
    }
    $std = [pscustomobject]@{
        Name = 'Rust standard library'; Version = $toolchain; License = 'MIT OR Apache-2.0, with the exceptions its inventory lists'
        Source = "https://github.com/rust-lang/rust/tree/$toolchain/library (COPYRIGHT: https://github.com/rust-lang/rust/blob/$toolchain/COPYRIGHT)"
        Note = 'Copyright The Rust Project Developers (see https://thanks.rust-lang.org) and the holders the inventory below names. The inventory is the toolchain''s own (share/doc/rust/COPYRIGHT-library.html, as text): the standard library''s release as a whole, the crates it is built from included.'
        Staged = $stdDir; Supplied = $null
    }

    $components = @((Get-Content (Join-Path $supplied 'components.json') -Raw | ConvertFrom-Json).components)
    Assert-ComponentTies $components (Get-Content (Join-Path $Source 'src-tauri/Cargo.lock') -Raw)
    $installer = @($components | ForEach-Object { ConvertTo-ComponentItem $_ $supplied })
    $sections = @(
        [pscustomobject]@{ Title = 'Rust standard library, compiled into luna.exe'; Items = @(Add-LicenseTexts @($std)) }
        [pscustomobject]@{ Title = 'Rust crates compiled into luna.exe'; Items = @(Add-LicenseTexts @($crates)) }
        [pscustomobject]@{ Title = 'Components carried outside the crate graph'; Items = @(Add-LicenseTexts $installer) }
    )

    # Surface license folders left behind by dependencies that no longer ship.
    $names = @($crates | ForEach-Object { $_.Name })
    $base = Join-Path $supplied 'crates'
    foreach ($folder in Get-ChildItem -LiteralPath $base -Directory) {
        if ($folder.Name -notin $names) { Write-Warning "third-party-licenses/crates/$($folder.Name) is for a crate that no longer ships: drop it" }
    }

    New-Item -ItemType Directory -Force (Split-Path $Out) | Out-Null
    [IO.File]::WriteAllText($Out, (New-NoticesText $version $sections), [Text.UTF8Encoding]::new($false))
    Write-Host "third-party notices ($(@($crates).Count) crates) -> $Out"
} finally {
    Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
}
