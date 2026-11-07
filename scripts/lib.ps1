# What the scripts in this folder share. Each one dot-sources it first:
#
#   . "$PSScriptRoot/lib.ps1"
#
# Everything here goes by $RepoRoot, never by the current directory, so every
# script runs the same from wherever it is started.
$ErrorActionPreference = 'Stop'

$RepoRoot = Split-Path -Parent $PSScriptRoot

# The commit a build is made from, resolved on the host rather than in build.rs:
# inside the container the repo's .git may be a worktree pointer at a path that
# only exists here. A trailing + means the tree had uncommitted changes.
function Get-BuildStamp {
    $stamp = (git -C $RepoRoot rev-parse --short HEAD)
    if ($LASTEXITCODE) { throw "git rev-parse failed (exit $LASTEXITCODE)" }
    $stamp = $stamp.Trim()
    if (git -C $RepoRoot status --porcelain) { $stamp = "$stamp+" }
    $stamp
}

# Runs a native command and throws when it fails. A native command's exit code
# does not trip $ErrorActionPreference, so without this a failed step would go
# on to the next one with whatever the last run left behind.
#
#   Invoke-Checked 'docker build' { docker build ... }
#
# The block looks its variables up through the call stack, this function's own
# parameters first: hence names no caller would use for a variable of its own.
function Invoke-Checked([string]$CheckedWhat, [scriptblock]$CheckedBlock) {
    & $CheckedBlock
    if ($LASTEXITCODE) { throw "$CheckedWhat failed (exit $LASTEXITCODE)" }
}

# The Node version everything runs on, from the repo's .node-version: the one
# place it is pinned, for the images here and for anyone's local Node.
function Get-NodeVersion {
    $version = (Get-Content (Join-Path $RepoRoot '.node-version') -TotalCount 1).Trim()
    if ($version -notmatch '^\d+\.\d+\.\d+$') { throw ".node-version should hold an exact x.y.z, not '$version'" }
    $version
}

# Builds one of the toolchain images in docker/ and returns the image to run.
# The tag is a hash of what the image is built from, the Dockerfile and its
# arguments: every checkout on the machine shares Docker, and under a plain name
# a checkout with another Dockerfile, building in between, would have this one
# run in its image. NODE_VERSION goes only to the images that take it (docker
# warns about an argument nothing uses).
function Build-Image([string]$Name, [string]$Dockerfile) {
    $file = Join-Path $RepoRoot "docker/$Dockerfile"
    $text = (Get-Content $file -Raw) -replace "`r`n", "`n"
    $argList = @()
    if ($text -match '(?m)^ARG NODE_VERSION') {
        $argList = @('--build-arg', "NODE_VERSION=$(Get-NodeVersion)")
    }
    $sha = [Security.Cryptography.SHA256]::Create()
    $bytes = [Text.Encoding]::UTF8.GetBytes($text + "`n" + ($argList -join ' '))
    $hash = -join ($sha.ComputeHash($bytes)[0..5] | ForEach-Object { $_.ToString('x2') })
    $image = "${Name}:$hash"
    Invoke-Checked "docker build of $image" {
        docker build -t $image @argList -f $file (Join-Path $RepoRoot 'docker') | Out-Host
    }
    $image
}

# A bash script for `bash -c`, with the CRLFs a Windows checkout gives a here-string
# taken out. The scripts keep double quotes out of these: Windows PowerShell breaks
# up a native argument with one in it.
function ConvertTo-BashScript([string]$Script) {
    $Script -replace "`r`n", "`n"
}

# The named volumes the Windows toolchain image (windows-build.Dockerfile) caches
# into. Every checkout on the machine shares them, hence Use-BuildLock.
$WinBuildVolumes = @(
    '-v', 'luna-node-modules:/app/node_modules',
    '-v', 'luna-xwin-cache:/xwin-cache',
    '-v', 'luna-cargo-registry:/usr/local/cargo/registry',
    '-v', 'luna-tauri-cache:/root/.cache/tauri'
)

# The test flavor's build environment (test-build.ps1, windows-tests.ps1), with
# a cargo target dir of its own in test-build/, so a test build never overwrites
# the release exe.
$TestBuildEnv = @(
    '-e', 'LUNA_FLAVOR=test',
    '-e', 'VITE_LUNA_FLAVOR=test',
    '-e', 'CARGO_TARGET_DIR=/app/test-build/target'
)

# Runs a bash script at the repo root in a Windows toolchain image, the checkout
# mounted at /app with the shared caches. $DockerArgs are extra `docker run`
# arguments (environment, mounts). Call it inside Use-BuildLock.
function Invoke-WinBuild([string]$What, [string]$Build, [string]$Image, [string]$Script, [string[]]$DockerArgs = @()) {
    $cmd = ConvertTo-BashScript "set -e`n$Script"
    Invoke-Checked $What {
        docker run --rm `
          -e COREPACK_ENABLE_DOWNLOAD_PROMPT=0 `
          -e LUNA_BUILD=$Build `
          @DockerArgs `
          -v "${RepoRoot}:/app" `
          @WinBuildVolumes `
          -w /app `
          $Image `
          bash -c $cmd
    }
}

# Runs a bash script in src-tauri/ in a Linux toolchain image (linux-check.Dockerfile),
# with the checkout read-only, so it leaves nothing behind there. Call it inside
# Use-BuildLock.
#
# Every checkout shares the target volume under the same /app path, and cargo goes
# by file times: a checkout whose sources are older than the last build would pass
# for up to date. So a script that builds the crate starts with
# `cargo clean -p luna`; its dependencies stay cached. A volume to each checkout
# would cost a full build and 6 GB apiece.
#
# That rebuild and what it makes are one step, under a lock in the volume itself:
# with another check's build landing between the two, this one would run that
# one's tests and report them as its own. Use-BuildLock cannot promise that: off
# Windows it stops at the user, and a Docker daemon may have several. This lock
# goes with the container, however it ends.
function Invoke-LinuxCargo([string]$What, [string]$Build, [string]$Image, [string]$Script) {
    $cmd = ConvertTo-BashScript @"
set -e
exec 9>/target/.linux-check.lock
flock -n 9 || { echo 'another Linux check is using these volumes; waiting for it'; flock 9; }
cd /app/src-tauri
$Script
"@
    # tauri-build writes gen/schemas, for this platform's view of them, on every
    # build: a tmpfs takes them. Its mount point has to be in the read-only
    # checkout already, and gen/ is not tracked.
    New-Item -ItemType Directory -Force (Join-Path $RepoRoot 'src-tauri/gen') | Out-Null
    # --init: the keeper's tests leave orphans to PID 1, and only a real init
    # reaps them; otherwise they stay zombies and look alive to kill(pid, 0).
    Invoke-Checked $What {
        docker run --rm --init `
          -e LUNA_BUILD=$Build `
          -e LUNA_FLAVOR=test `
          -e CARGO_TARGET_DIR=/target `
          -v "${RepoRoot}:/app:ro" `
          --tmpfs /app/src-tauri/gen `
          -v luna-linux-target:/target `
          -v luna-linux-cargo-registry:/usr/local/cargo/registry `
          -w /app `
          $Image `
          bash -c $cmd
    }
}

# The Rust toolchain is pinned twice, in src-tauri/rust-toolchain.toml and in the
# images' FROM lines, and rust-version in Cargo.toml names it a third time; Node
# in .node-version. Throws when any two disagree, naming the files: a bump of one
# alone (Dependabot moves the FROM lines) would otherwise build with a toolchain
# rustup fetches on the fly.
function Assert-ToolchainPins {
    $toolchain = Get-Content (Join-Path $RepoRoot 'src-tauri/rust-toolchain.toml') -Raw
    if ($toolchain -notmatch '(?m)^channel\s*=\s*"(\d+\.\d+\.\d+)"') {
        throw 'src-tauri/rust-toolchain.toml should pin an exact channel = "x.y.z"'
    }
    $rust = $Matches[1]
    $problems = @()
    $manifest = Get-Content (Join-Path $RepoRoot 'src-tauri/Cargo.toml') -Raw
    if ($manifest -notmatch "(?m)^rust-version\s*=\s*`"$([regex]::Escape($rust))`"") {
        $problems += "src-tauri/Cargo.toml rust-version is not $rust"
    }
    foreach ($file in Get-ChildItem (Join-Path $RepoRoot 'docker') -Filter *.Dockerfile) {
        foreach ($from in Select-String -Path $file.FullName -Pattern '^FROM rust:(\S+)') {
            if ($from.Matches[0].Groups[1].Value -notmatch "^$([regex]::Escape($rust))-") {
                $problems += "docker/$($file.Name) has $($from.Line), not rust:$rust-*"
            }
        }
    }
    if ($problems) {
        throw "the Rust pins disagree with rust-toolchain.toml ($rust): $($problems -join '; ')"
    }
    Write-Host "Rust $rust, Node $(Get-NodeVersion)"
}

# Runs $Body as the only Luna build on this machine, from any checkout: the named
# volumes are shared, and two builds setting up the MSVC CRT in luna-xwin-cache at
# once break each other ("Failed to setup MSVC CRT"), as two pnpm installs into
# luna-node-modules would. The Linux volumes are linux-check's own, but the
# machine's CPU and Docker's disk are not. Off Windows a named mutex reaches only
# the user's own processes; the Linux check adds a lock of its own for that.
function Use-BuildLock([scriptblock]$Body) {
    $lock = [System.Threading.Mutex]::new($false, 'Global\luna-docker-build')
    $held = $false
    try {
        try {
            if (-not $lock.WaitOne(0)) {
                Write-Host 'another Luna build is running; waiting for it'
                [void]$lock.WaitOne()
            }
        } catch [System.Threading.AbandonedMutexException] {
            # A build that died holding it: the mutex is ours now.
        }
        $held = $true
        & $Body
    } finally {
        if ($held) { $lock.ReleaseMutex() }
        $lock.Dispose()
    }
}
