#Requires -Version 7.4
# The release's CycloneDX JSON for the Windows app's Cargo dependencies.
# The workspace is copied into the container so cargo-cyclonedx can resolve
# the path dependencies and write its per-crate reports off the checkout.
#
#   ./scripts/sbom.ps1 -Out <dir>    luna-<version>-cargo.cdx.json
#
# cargo-cyclonedx is pinned and built with its own lockfile.
param([Parameter(Mandatory)][string]$Out)
. "$PSScriptRoot/lib.ps1"
. "$PSScriptRoot/release-lib.ps1"

$CargoCycloneDx = '0.5.9'

$version = Get-ReleaseVersion
$Out = [IO.Path]::GetFullPath($Out, (Get-Location).Path)
New-Item -ItemType Directory -Force $Out | Out-Null

# The Rust image the Windows toolchain image builds on, by the same tag and
# digest: its cargo is the pinned toolchain's, and it needs nothing else.
$rust = Get-BaseImages | Where-Object { $_.File -eq 'docker/windows-build.Dockerfile' -and $_.Repo -eq 'rust' }
if (@($rust).Count -ne 1) { throw 'docker/windows-build.Dockerfile should have one FROM rust: line' }

Use-BuildLock {
    # The Rust image already has the pinned toolchain. Use it without
    # fetching the cross-compiler components for this metadata-only command.
    Invoke-Checked 'Cargo SBOM' {
        docker run --rm `
          -e RUSTUP_TOOLCHAIN=$($rust.Tag -replace '-.*$', '') `
          -e CARGO_TERM_COLOR=never `
          -v "${RepoRoot}:/app:ro" `
          -v "${Out}:/out" `
          -v luna-cargo-registry:/usr/local/cargo/registry `
          $rust.Ref `
          bash -c (ConvertTo-BashScript @"
set -eo pipefail
cargo install --locked --quiet cargo-cyclonedx --version $CargoCycloneDx
mkdir -p /work
tar -C /app --exclude=src-tauri/target -cf - src-tauri | tar -C /work -xf -
cd /work/src-tauri
cargo cyclonedx --format json --spec-version 1.5 --target x86_64-pc-windows-msvc --override-filename sbom
cmp -s Cargo.lock /app/src-tauri/Cargo.lock || { echo 'Cargo.lock is not up to date with Cargo.toml'; exit 1; }
# The root package is the one shipped in the installer. Its BOM includes
# the window crate; other workspace reports are kept inside the container.
cp sbom.json /out/luna-$version-cargo.cdx.json
"@)
    }
}

# Check the file before the installer is built and signed for the release.
$file = Join-Path $Out "luna-$version-cargo.cdx.json"
if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { throw "the Cargo SBOM was not written to $file" }
Assert-AttestableSbom $file
Write-Host "SBOM -> $file"
