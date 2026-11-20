#Requires -Version 7.4
# The steps of the gate: formatting, lints, types, tests and the frontend build,
# in Docker on the pinned toolchains, and PowerShell checks on the host. CI runs
# this same script, a group per
# job (.github/workflows/ci.yml), and CI on GitHub is the gate a change passes
# (scripts/ci-wait.ps1 reads it); a run here is for narrowing a failure down,
# one group at a time, as it holds the machine's build lock throughout.
#
#   ./scripts/gate.ps1                    the scripts, frontend and Linux Rust checks
#   ./scripts/gate.ps1 -Windows           and clippy over the Windows build too
#   ./scripts/gate.ps1 -Group scripts     PowerShell checks, with no Docker
#
# Stops at the first step that fails and ends on a summary of every step. The
# Windows unit tests are scripts/windows-tests.ps1: they run on a Windows host,
# not in a container.
param(
    [switch]$Windows,
    [ValidateSet('scripts', 'frontend', 'rust', 'windows')]
    [string[]]$Group = @()
)
. "$PSScriptRoot/lib.ps1"

if (-not $Group) {
    $Group = @('scripts', 'frontend', 'rust')
    if ($Windows) { $Group += 'windows' }
}

$build = Get-BuildStamp

# A package.json script, in the Node stage of the Windows image (Get-FrontendImage):
# the frontend needs nothing else. pnpm itself is the one packageManager names,
# through corepack, whose downloads stay in a volume of their own.
function Invoke-Frontend([string]$What, [string]$Script) {
    Initialize-HostDirs 'test-build', 'node_modules'
    Invoke-Checked $What {
        docker run --rm `
          -e COREPACK_ENABLE_DOWNLOAD_PROMPT=0 `
          -e COREPACK_HOME=/corepack `
          -v "${RepoRoot}:/app" `
          -v "$(Get-NodeModulesVolume):/app/node_modules" `
          -v luna-corepack:/corepack `
          -w /app `
          $images.node `
          bash -c (ConvertTo-BashScript "set -e`n$Script")
    }
}

# A step that runs a package.json script.
function New-ScriptStep([string]$Name) {
    @{
        Name   = "pnpm $Name"
        Group  = 'frontend'
        Script = $Name
        Run    = { param($step) Invoke-Frontend $step.Name "corepack pnpm run $($step.Script)" }
    }
}

# The Rust steps share linux-check's image, volumes and lock (Invoke-LinuxCargo),
# and rebuild the crate itself first, for the reason given there. The images are
# steps of their own, so a broken Dockerfile shows as that.
$images = @{}
$steps = @(
    @{
        Name  = 'toolchain pins agree'
        Group = 'frontend', 'rust', 'windows'
        Run   = { Assert-ToolchainPins }
    }
    @{
        Name  = 'Node toolchain image'
        Group = 'frontend'
        Run   = { $images.node = Get-FrontendImage }
    }
    @{
        Name  = 'pnpm install --frozen-lockfile'
        Group = 'frontend'
        Run   = { Invoke-Frontend 'pnpm install' 'corepack pnpm install --frozen-lockfile' }
    }
    New-ScriptStep 'lint'
    New-ScriptStep 'format:check'
    New-ScriptStep 'typecheck'
    @{
        Name  = 'check-text tests and tracked files'
        Group = 'scripts'
        Run   = {
            Invoke-Checked 'check-text tests' { pwsh -NoProfile -File (Join-Path $RepoRoot 'scripts/check-text.test.ps1') }
        }
    }
    New-ScriptStep 'test:scripts'
    @{
        # The release scripts' logic (release-lib.ps1) is PowerShell, run on
        # the host like the scripts themselves; a pwsh of its own keeps the
        # tests' state out of the gate's.
        Name  = 'release-lib tests'
        Group = 'scripts'
        Run   = {
            Invoke-Checked 'release-lib tests' { pwsh -NoProfile -File (Join-Path $RepoRoot 'scripts/release-lib.test.ps1') }
        }
    }
    @{
        # What the third-party notices are made of (notices-lib.ps1), on a
        # made-up tree: the notices script itself needs the registries.
        Name  = 'notices-lib tests'
        Group = 'scripts'
        Run   = {
            Invoke-Checked 'notices-lib tests' { pwsh -NoProfile -File (Join-Path $RepoRoot 'scripts/notices-lib.test.ps1') }
        }
    }
    @{
        # What ci-wait.ps1 decides on (the run for a commit, the failed jobs),
        # on sample JSON: the script itself needs GitHub.
        Name  = 'ci-wait-lib tests'
        Group = 'scripts'
        Run   = {
            Invoke-Checked 'ci-wait-lib tests' { pwsh -NoProfile -File (Join-Path $RepoRoot 'scripts/ci-wait-lib.test.ps1') }
        }
    }
    @{
        # The signer's container writes the .sig into a folder of the host
        # user's, as in the release's sign job; on a Linux host, CI's, this is
        # what that job meets (Assert-SignerWritesBundle).
        Name  = 'signer writes into the bundle'
        Group = 'frontend'
        Run   = {
            . (Join-Path $RepoRoot 'scripts/release-lib.ps1')
            Assert-SignerWritesBundle
        }
    }
    New-ScriptStep 'test'
    New-ScriptStep 'build'
    @{
        # What the notices list beyond the lockfiles' packages, against the tree:
        # each entry of third-party-licenses/components.json still at the version
        # it was checked against, and every helper the build tools wrote into
        # the bundle just built listed, and nothing listed that it lacks.
        Name  = 'third-party components listed'
        Group = 'frontend'
        Run   = {
            . (Join-Path $RepoRoot 'scripts/notices-lib.ps1')
            $components = @((Get-Content (Join-Path $RepoRoot 'third-party-licenses/components.json') -Raw | ConvertFrom-Json).components)
            Assert-ComponentTies $components (Get-Content (Join-Path $RepoRoot 'src-tauri/Cargo.lock') -Raw) (Get-Content (Join-Path $RepoRoot 'pnpm-lock.yaml') -Raw)
            Assert-BundledHelpers (Join-Path $RepoRoot 'dist') $components
        }
    }
    @{
        Name  = 'Linux toolchain image'
        Group = 'rust'
        Run   = { $images.linux = Build-Image luna-linuxcheck linux-check.Dockerfile }
    }
    @{
        # The commit-message check's own tests. Its range mode runs git, which
        # the slim Node image lacks, so they run in the Linux toolchain image;
        # they make a throwaway repository in the container's /tmp and touch no
        # volume, so no cargo lock is needed. CI's frontend job runs them again
        # on the runner's sh, before checking the pushed range.
        Name  = 'check-commit-msg tests'
        Group = 'rust'
        Run   = {
            Invoke-Checked 'check-commit-msg tests' {
                docker run --rm -v "${RepoRoot}:/app:ro" -w /app $images.linux sh scripts/check-commit-msg.test.sh
            }
        }
    }
    @{
        # The macOS bundle script (scripts/macos-app.sh), dry-run here; CI's
        # macOS runner makes the real bundle. No volume, no lock.
        Name  = 'macos-app tests'
        Group = 'rust'
        Run   = {
            Invoke-Checked 'macos-app tests' {
                docker run --rm -v "${RepoRoot}:/app:ro" -w /app $images.linux sh scripts/macos-app.test.sh
            }
        }
    }
    @{
        Name  = 'cargo fmt --check'
        Group = 'rust'
        # The app and its window crate, by name: a plain `cargo fmt` would
        # take the root package alone, and `--all` the vendored crates too.
        Run   = { Invoke-LinuxCargo 'cargo fmt' $build $images.linux 'cargo fmt -p luna -p luna-window -- --check' }
    }
    @{
        Name  = 'cargo clippy (Linux)'
        Group = 'rust'
        Run   = {
            # The screenshot harness's code only builds with its feature.
            Invoke-LinuxCargo 'cargo clippy' $build $images.linux @'
cargo clean --locked -p luna -p luna-window
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo clippy --locked -p luna-window --features shots --all-targets -- -D warnings
'@
        }
    }
    @{
        Name  = 'cargo test (Linux)'
        Group = 'rust'
        Run   = {
            Invoke-LinuxCargo 'cargo test' $build $images.linux @'
cargo clean --locked -p luna -p luna-window
cargo test --locked --workspace --lib --bins
'@
        }
    }
    @{
        Name  = 'Windows toolchain image'
        Group = 'windows'
        Run   = { $images.windows = Build-Image luna-winbuild windows-build.Dockerfile }
    }
    @{
        Name  = 'cargo clippy (Windows)'
        Group = 'windows'
        # The test build's target dir: this checkout's own, and already warm.
        Run   = {
            Invoke-WinBuild 'cargo xwin clippy' $build $images.windows @'
cd src-tauri
cargo xwin clippy --locked --workspace --all-targets --target x86_64-pc-windows-msvc -- -D warnings
cargo xwin clippy --locked -p luna-window --features shots --all-targets --target x86_64-pc-windows-msvc -- -D warnings
'@ $TestBuildEnv
        }
    }
)

$results = @()
$failed = $null
$runSteps = {
    foreach ($step in $steps) {
        if (-not ($step.Group | Where-Object { $_ -in $Group })) { continue }
        if ($failed) {
            $script:results += [pscustomobject]@{ Step = $step.Name; Result = 'not run'; Time = '' }
            continue
        }
        Write-Host "== $($step.Name)" -ForegroundColor Cyan
        $timer = [Diagnostics.Stopwatch]::StartNew()
        $result = 'ok'
        try {
            & $step.Run $step
        } catch {
            $result = "FAILED: $_"
            $script:failed = $step.Name
        }
        $script:results += [pscustomobject]@{
            Step   = $step.Name
            Result = $result
            Time   = '{0:mm\:ss}' -f $timer.Elapsed
        }
    }
}
if ($Group.Count -eq 1 -and $Group[0] -eq 'scripts') {
    & $runSteps
} else {
    Use-BuildLock $runSteps
}

$results | Format-Table -AutoSize | Out-String -Width 200 | Write-Host
if ($failed) {
    Write-Host "gate failed at: $failed" -ForegroundColor Red
    exit 1
}
Write-Host "gate passed ($($Group -join ', '))" -ForegroundColor Green
