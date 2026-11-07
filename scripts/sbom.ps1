#Requires -Version 7.4
# The release's software bills of materials, CycloneDX JSON: the crates the
# Windows build compiles in (cargo-cyclonedx) and the npm packages its frontend
# bundles (cyclonedx-npm). Each is made in Docker from the lockfiles alone,
# with the checkout mounted read-only, so nothing is left behind there.
#
#   ./scripts/sbom.ps1 -Out <dir>    luna-<version>-cargo.cdx.json and luna-<version>-npm.cdx.json
#
# The tools are pinned here, at releases some months old rather than ones out
# for days. cargo-cyclonedx is built with its own lockfile (cargo install
# --locked); cyclonedx-npm is pinned itself, and npx resolves its dependencies.
param([Parameter(Mandatory)][string]$Out)
. "$PSScriptRoot/lib.ps1"
. "$PSScriptRoot/release-lib.ps1"

$CargoCycloneDx = '0.5.9'
$CycloneDxNpm = '6.0.1'

$version = Get-ReleaseVersion
$Out = [IO.Path]::GetFullPath($Out, (Get-Location).Path)
New-Item -ItemType Directory -Force $Out | Out-Null

# The Rust image the Windows toolchain image builds on, by the same tag and
# digest: its cargo is the pinned toolchain's, and it needs nothing else.
$rust = Get-BaseImages | Where-Object { $_.File -eq 'docker/windows-build.Dockerfile' -and $_.Repo -eq 'rust' }
if (@($rust).Count -ne 1) { throw 'docker/windows-build.Dockerfile should have one FROM rust: line' }

Use-BuildLock {
    # The crate as the Windows target sees it: dependencies of other platforms
    # (the GTK stack, libc's Unix side) are not in the exe. A copy of the
    # manifest, lockfile and sources, without rust-toolchain.toml, which would
    # have rustup fetch the Windows target for nothing. A lockfile cargo had to
    # change means the SBOM would not describe what `--locked` builds.
    Invoke-Checked 'Cargo SBOM' {
        docker run --rm `
          -e CARGO_TERM_COLOR=never `
          -v "${RepoRoot}:/app:ro" `
          -v "${Out}:/out" `
          -v luna-cargo-registry:/usr/local/cargo/registry `
          $rust.Ref `
          bash -c (ConvertTo-BashScript @"
set -e
cargo install --locked --quiet cargo-cyclonedx --version $CargoCycloneDx
mkdir -p /work/src-tauri
cp -r /app/src-tauri/Cargo.toml /app/src-tauri/Cargo.lock /app/src-tauri/build.rs /app/src-tauri/src /work/src-tauri/
cd /work/src-tauri
cargo cyclonedx --format json --spec-version 1.5 --target x86_64-pc-windows-msvc --override-filename sbom
cmp -s Cargo.lock /app/src-tauri/Cargo.lock || { echo 'Cargo.lock is not up to date with Cargo.toml'; exit 1; }
cp sbom.json /out/luna-$version-cargo.cdx.json
"@)
    }

    # The packages the frontend bundle is built from: production dependencies
    # only, the build tools being in the devDependencies. cyclonedx-npm reads
    # the tree through npm, which cannot follow pnpm's own layout, so pnpm lays
    # it out flat (node-linker=hoisted) from the same lockfile. No install
    # scripts run; nothing here is built.
    Invoke-Checked 'npm SBOM' {
        docker run --rm `
          -e COREPACK_ENABLE_DOWNLOAD_PROMPT=0 `
          -e COREPACK_HOME=/corepack `
          -e npm_config_update_notifier=false `
          -v "${RepoRoot}:/app:ro" `
          -v "${Out}:/out" `
          -v luna-corepack:/corepack `
          (Get-NodeImage) `
          bash -c (ConvertTo-BashScript @"
set -e
mkdir /work
cp /app/package.json /app/pnpm-lock.yaml /work/
cd /work
corepack pnpm install --frozen-lockfile --prod --ignore-scripts --config.node-linker=hoisted --reporter=silent
npx --yes @cyclonedx/cyclonedx-npm@$CycloneDxNpm --omit dev --spec-version 1.5 --mc-type application --output-reproducible --validate --output-format JSON --output-file /out/luna-$version-npm.cdx.json
"@)
    }
}

Get-ChildItem -LiteralPath $Out -Filter "luna-$version-*.cdx.json" | ForEach-Object { Write-Host "SBOM -> $($_.FullName)" }
