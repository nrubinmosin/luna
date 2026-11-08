#Requires -Version 7.4
# The one gate a change passes before it goes in: formatting, lints, types, tests
# and the frontend build, each in Docker on the pinned toolchains. CI runs this
# same script, a group per job (.github/workflows/ci.yml), so a green run here is
# the green run there.
#
#   ./scripts/gate.ps1                    the frontend and the Linux Rust checks
#   ./scripts/gate.ps1 -Windows           and clippy over the Windows build too
#   ./scripts/gate.ps1 -Group frontend    one group only (rust, windows likewise)
#
# Stops at the first step that fails and ends on a summary of every step. The
# Windows unit tests are scripts/windows-tests.ps1: they run on a Windows host,
# not in a container.
param(
    [switch]$Windows,
    [ValidateSet('frontend', 'rust', 'windows')]
    [string[]]$Group = @()
)
. "$PSScriptRoot/lib.ps1"

if (-not $Group) {
    $Group = @('frontend', 'rust')
    if ($Windows) { $Group += 'windows' }
}

$build = Get-BuildStamp
$nodeImage = Get-NodeImage

# A package.json script, in the official Node image of the pinned version and
# digest: the frontend needs nothing else. pnpm itself is the one packageManager names,
# through corepack, whose downloads stay in a volume of their own.
function Invoke-Frontend([string]$What, [string]$Script) {
    Initialize-HostDirs 'test-build', 'node_modules'
    Invoke-Checked $What {
        docker run --rm `
          -e COREPACK_ENABLE_DOWNLOAD_PROMPT=0 `
          -e COREPACK_HOME=/corepack `
          -v "${RepoRoot}:/app" `
          -v luna-node-modules:/app/node_modules `
          -v luna-corepack:/corepack `
          -w /app `
          $nodeImage `
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
        Group = '*'
        Run   = { Assert-ToolchainPins }
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
        # check:text asks git for the tracked files, and the container has no
        # git; nor could one read a worktree's .git, a pointer to a host path.
        # So the host lists them (pwsh 7.4+ redirects a native command's bytes
        # untouched, NULs and all), and xargs hands the list on.
        Name  = 'pnpm check:text'
        Group = 'frontend'
        Run   = {
            $list = Join-Path $RepoRoot 'test-build/tracked-files'
            New-Item -ItemType Directory -Force (Split-Path $list) | Out-Null
            Invoke-Checked 'git ls-files' { git -C $RepoRoot ls-files -z > $list }
            Invoke-Frontend 'pnpm check:text' 'xargs -0 -a test-build/tracked-files corepack pnpm run check:text'
        }
    }
    # tauri build refuses a Tauri crate and its npm package on different
    # major.minor lines, and nothing here runs tauri build.
    New-ScriptStep 'check:tauri'
    New-ScriptStep 'test:scripts'
    @{
        # The release scripts' logic (release-lib.ps1) is PowerShell, run on
        # the host like the scripts themselves; a pwsh of its own keeps the
        # tests' state out of the gate's.
        Name  = 'release-lib tests'
        Group = 'frontend'
        Run   = {
            Invoke-Checked 'release-lib tests' { pwsh -NoProfile -File (Join-Path $RepoRoot 'scripts/release-lib.test.ps1') }
        }
    }
    @{
        # What the third-party notices are made of (notices-lib.ps1), on a
        # made-up tree: the notices script itself needs the registries.
        Name  = 'notices-lib tests'
        Group = 'frontend'
        Run   = {
            Invoke-Checked 'notices-lib tests' { pwsh -NoProfile -File (Join-Path $RepoRoot 'scripts/notices-lib.test.ps1') }
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
        Name  = 'cargo fmt --check'
        Group = 'rust'
        Run   = { Invoke-LinuxCargo 'cargo fmt' $build $images.linux 'cargo fmt --check' }
    }
    @{
        Name  = 'cargo clippy (Linux)'
        Group = 'rust'
        Run   = {
            Invoke-LinuxCargo 'cargo clippy' $build $images.linux @'
cargo clean --locked -p luna
cargo clippy --locked --all-targets -- -D warnings
'@
        }
    }
    @{
        Name  = 'cargo test (Linux)'
        Group = 'rust'
        Run   = {
            Invoke-LinuxCargo 'cargo test' $build $images.linux @'
cargo clean --locked -p luna
cargo test --locked --lib --bins
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
        # The crate embeds the frontend's dist/; an empty one does for a lint.
        # The test build's target dir: this checkout's own, and already warm.
        Run   = {
            Invoke-WinBuild 'cargo xwin clippy' $build $images.windows @'
mkdir -p dist
cd src-tauri
cargo xwin clippy --locked --all-targets --target x86_64-pc-windows-msvc -- -D warnings
'@ $TestBuildEnv
        }
    }
)

$results = @()
$failed = $null
Use-BuildLock {
    foreach ($step in $steps) {
        if ($step.Group -ne '*' -and $step.Group -notin $Group) { continue }
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

$results | Format-Table -AutoSize | Out-String -Width 200 | Write-Host
if ($failed) {
    Write-Host "gate failed at: $failed" -ForegroundColor Red
    exit 1
}
Write-Host "gate passed ($($Group -join ', '))" -ForegroundColor Green
