#Requires -Version 7.4
# The steps of the gate: formatting, lints and Rust tests in Docker, and
# PowerShell checks on the host. CI runs this script one group per job
# (.github/workflows/ci.yml), and CI on GitHub is the gate a change passes
# (scripts/ci-wait.ps1 reads it); a run here is for narrowing a failure down,
# one group at a time, as it holds the machine's build lock throughout.
#
#   ./scripts/gate.ps1                    the scripts and Linux Rust checks
#   ./scripts/gate.ps1 -Windows           and clippy over the Windows build too
#   ./scripts/gate.ps1 -Group scripts     script checks, with no Docker
#
# Stops at the first step that fails and ends on a summary of every step. The
# Windows unit tests are scripts/windows-tests.ps1: they run on a Windows host,
# not in a container.
param(
    [switch]$Windows,
    [ValidateSet('scripts', 'rust', 'windows')]
    [string[]]$Group = @()
)
. "$PSScriptRoot/lib.ps1"

if (-not $Group) {
    $Group = @('scripts', 'rust')
    if ($Windows) { $Group += 'windows' }
}

$build = Get-BuildStamp

# The Rust steps share linux-check's image, volumes and lock (Invoke-LinuxCargo),
# and rebuild the crate itself first, for the reason given there. The images are
# steps of their own, so a broken Dockerfile shows as that.
$images = @{}
$steps = @(
    @{
        Name  = 'toolchain pins agree'
        Group = 'scripts', 'rust', 'windows'
        Run   = { Assert-ToolchainPins }
    }
    @{
        Name  = 'check-text tests and tracked files'
        Group = 'scripts'
        Run   = {
            Invoke-Checked 'check-text tests' { pwsh -NoProfile -File (Join-Path $RepoRoot 'scripts/check-text.test.ps1') }
        }
    }
    @{
        Name  = 'changelog fragments and check-changes tests'
        Group = 'scripts'
        Run   = {
            Invoke-Checked 'check-changes' { pwsh -NoProfile -File (Join-Path $RepoRoot 'scripts/check-changes.ps1') }
            Invoke-Checked 'check-changes tests' { pwsh -NoProfile -File (Join-Path $RepoRoot 'scripts/check-changes.test.ps1') }
        }
    }
    @{
        Name  = 'shell script tests'
        Group = 'scripts'
        Run   = {
            $shell = (Get-Command sh -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1).Source
            if (-not $shell -and $IsWindows) {
                # Git for Windows puts git on PATH without its Unix tools.
                $gitRoot = Join-Path (git --exec-path) '../../..' -Resolve
                $shell = Join-Path $gitRoot 'bin/sh.exe'
            }
            if (-not $shell -or -not (Test-Path -LiteralPath $shell -PathType Leaf)) {
                throw 'shell script tests need sh (included with Git for Windows)'
            }
            Invoke-Checked 'check-commit-msg tests' { & $shell (Join-Path $RepoRoot 'scripts/check-commit-msg.test.sh') }
            Invoke-Checked 'docker-msys tests' { & $shell (Join-Path $RepoRoot '.agents/hooks/docker-msys.test.sh') }
        }
    }
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
        # How live-tests.ps1 finds the test build's managed CLIs, on made-up
        # trees: the live suite itself needs signed-in accounts.
        Name  = 'live-tests-lib tests'
        Group = 'scripts'
        Run   = {
            Invoke-Checked 'live-tests-lib tests' { pwsh -NoProfile -File (Join-Path $RepoRoot 'scripts/live-tests-lib.test.ps1') }
        }
    }
    @{
        Name  = 'third-party components listed'
        Group = 'scripts'
        Run   = {
            . (Join-Path $RepoRoot 'scripts/notices-lib.ps1')
            $components = @((Get-Content (Join-Path $RepoRoot 'third-party-licenses/components.json') -Raw | ConvertFrom-Json).components)
            Assert-ComponentTies $components (Get-Content (Join-Path $RepoRoot 'src-tauri/Cargo.lock') -Raw)
        }
    }
    @{
        Name  = 'Linux toolchain image'
        Group = 'rust'
        Run   = { $images.linux = Build-Image luna-linuxcheck linux-check.Dockerfile }
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
