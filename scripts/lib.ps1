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

# The base images the Dockerfiles in docker/ build on, one per FROM line:
# File, Line, Repo (rust, node), Tag and Digest. Each FROM names its image by
# tag and digest, the tag for people and Dependabot, the digest so the tag
# cannot move under a build.
function Get-BaseImages {
    foreach ($file in Get-ChildItem (Join-Path $RepoRoot 'docker') -Filter *.Dockerfile) {
        foreach ($from in Select-String -Path $file.FullName -Pattern '^FROM\s+(\S+)') {
            $ref = $from.Matches[0].Groups[1].Value
            $parsed = $ref -match '^(?<repo>[^:@]+):(?<tag>[^@]+)(@(?<digest>sha256:[0-9a-f]{64}))?$'
            [pscustomobject]@{
                File   = "docker/$($file.Name)"
                Line   = $from.Line
                Ref    = $ref
                Repo   = if ($parsed) { $Matches.repo }
                Tag    = if ($parsed) { $Matches.tag }
                Digest = if ($parsed) { $Matches.digest }
            }
        }
    }
}

# The Node image the frontend steps run in: the Windows image's own Node stage,
# so the one pin (tag and digest, which Dependabot moves) serves both.
function Get-NodeImage {
    $node = Get-BaseImages | Where-Object { $_.File -eq 'docker/windows-build.Dockerfile' -and $_.Repo -eq 'node' }
    if (@($node).Count -ne 1) { throw 'docker/windows-build.Dockerfile should have one FROM node: line' }
    $node.Ref
}

# Builds one of the toolchain images in docker/ and returns the image to run.
# The tag is a hash of what the image is built from, the Dockerfile: every
# checkout on the machine shares Docker, and under a plain name a checkout with
# another Dockerfile, building in between, would have this one run in its image.
function Build-Image([string]$Name, [string]$Dockerfile) {
    $file = Join-Path $RepoRoot "docker/$Dockerfile"
    $text = (Get-Content $file -Raw) -replace "`r`n", "`n"
    $sha = [Security.Cryptography.SHA256]::Create()
    $hash = -join ($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($text))[0..5] | ForEach-Object { $_.ToString('x2') })
    $image = "${Name}:$hash"
    Invoke-Checked "docker build of $image" {
        docker build -t $image -f $file (Join-Path $RepoRoot 'docker') | Out-Host
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
# in .node-version and the FROM lines. Throws when any two disagree, or a FROM
# line has no digest, naming the files: a bump of one alone (Dependabot moves the
# FROM lines) would otherwise build with a toolchain rustup fetches on the fly,
# or a frontend checked on another Node than the one people run.
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
    $node = Get-NodeVersion
    foreach ($base in Get-BaseImages) {
        if (-not $base.Digest) {
            $problems += "$($base.File) has $($base.Line), with no @sha256: digest"
        } elseif ($base.Repo -eq 'rust' -and $base.Tag -notmatch "^$([regex]::Escape($rust))-") {
            $problems += "$($base.File) has $($base.Line), not rust:$rust-* (rust-toolchain.toml)"
        } elseif ($base.Repo -eq 'node' -and $base.Tag -notmatch "^$([regex]::Escape($node))-") {
            $problems += "$($base.File) has $($base.Line), not node:$node-* (.node-version)"
        }
    }
    if ($problems) {
        throw "the toolchain pins disagree: $($problems -join '; ')"
    }
    Write-Host "Rust $rust, Node $node, base images pinned by digest"
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
