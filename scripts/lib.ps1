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
    # Empty output means a clean tree only when git ran at all.
    $changes = git -C $RepoRoot status --porcelain
    if ($LASTEXITCODE) { throw "git status failed (exit $LASTEXITCODE)" }
    if ($changes) { $stamp = "$stamp+" }
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

# The base images the Dockerfiles in docker/ build on, one per FROM line:
# File, Line, Repo, Tag and Digest. Each FROM names its image by
# tag and digest, the tag for people, the digest so the tag cannot move under a
# build; both are bumped by hand (docs/adr/0029-dependencies-move-by-hand.md).
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

# Builds one of the toolchain images in docker/
# and returns the image to run. The tag is a hash of what the image is built
# from, the Dockerfile: every checkout on the machine shares Docker, and under a
# plain name a checkout with another Dockerfile, building in between, would have
# this one run in its image.
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
function Get-WinBuildVolumes {
    @(
        '-v', 'luna-xwin-cache:/xwin-cache',
        '-v', 'luna-cargo-registry:/usr/local/cargo/registry'
    )
}

# The test flavor's build environment (test-build.ps1, windows-tests.ps1), with
# a cargo target dir of its own in test-build/, so a test build never overwrites
# the release exe.
$TestBuildEnv = @(
    '-e', 'LUNA_FLAVOR=test',
    '-e', 'CARGO_TARGET_DIR=/app/test-build/target'
)

# The directories a container run mounts into, or makes inside, the checkout.
# The containers run as root (the toolchain images keep their caches under root's
# cargo and home), so on a Linux host whatever the container creates there is
# root-owned, and the host user could then neither write into it nor clear it:
# a CI runner failed on test-build/ exactly so. Docker Desktop maps ownership and
# never shows it. Making them first, as the invoking user, leaves root free to
# write in them and the user free to as well. Running the containers as the host
# user instead would need every cache volume and /usr/local/cargo to be theirs.
function Initialize-HostDirs([string[]]$Names = @('test-build', 'src-tauri/target'), [string]$Root = $RepoRoot) {
    foreach ($name in $Names) {
        New-Item -ItemType Directory -Force (Join-Path $Root $name) | Out-Null
    }
}

# LNK4099 is lld-link saying the MSVC CRT import libraries ship without debug
# info; it comes by the hundred and means nothing for us. Only that warning is
# ignored, and only for the MSVC target, which is the one these builds make.
# Runs a bash script at the repo root in a Windows toolchain image, the checkout
# (or the tree $Root names) mounted at /app with the shared caches. $DockerArgs
# are extra `docker run` arguments (environment, mounts). Call it inside
# Use-BuildLock.
function Invoke-WinBuild([string]$What, [string]$Build, [string]$Image, [string]$Script, [string[]]$DockerArgs = @(), [string]$Root = $RepoRoot) {
    $cmd = ConvertTo-BashScript "set -e`n$Script"
    Initialize-HostDirs -Root $Root
    $volumes = Get-WinBuildVolumes
    Invoke-Checked $What {
        docker run --rm `
          -e LUNA_BUILD=$Build `
          -e CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS="-C link-arg=/IGNORE:4099" `
          @DockerArgs `
          -v "${Root}:/app" `
          @volumes `
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
    # --init: the keeper's tests leave orphans to PID 1, and only a real init
    # reaps them; otherwise they stay zombies and look alive to kill(pid, 0).
    Invoke-Checked $What {
        docker run --rm --init `
          -e LUNA_BUILD=$Build `
          -e LUNA_FLAVOR=test `
          -e CARGO_TARGET_DIR=/target `
          -v "${RepoRoot}:/app:ro" `
          -v luna-linux-target:/target `
          -v luna-linux-cargo-registry:/usr/local/cargo/registry `
          -w /app `
          $Image `
          bash -c $cmd
    }
}

# The Rust toolchain is pinned in rust-toolchain.toml, the Docker images and
# Cargo.toml. Agreement keeps rustup from fetching another toolchain at build
# time. Each base image also needs a digest to pin its contents.
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
    foreach ($base in Get-BaseImages) {
        if (-not $base.Digest) {
            $problems += "$($base.File) has $($base.Line), with no @sha256: digest"
        } elseif ($base.Repo -eq 'rust' -and $base.Tag -notmatch "^$([regex]::Escape($rust))-") {
            $problems += "$($base.File) has $($base.Line), not rust:$rust-* (rust-toolchain.toml)"
        }
    }
    if ($problems) {
        throw "the toolchain pins disagree: $($problems -join '; ')"
    }
    Write-Host "Rust $rust, base images pinned by digest"
}

# Runs $Body as the only Luna build on this machine, from any checkout: the named
# volumes are shared, and two builds setting up the MSVC CRT in luna-xwin-cache at
# once break each other ("Failed to setup MSVC CRT"). Linux builds have their
# own volumes but share the machine's CPU and Docker's disk. Off Windows a named
# mutex reaches only the user's own processes, so the Linux check also locks its
# volumes.
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

# Whether an account folder holds a login its CLI can start on, by the rule
# Luna's own spawns go by (agents/spawn/account.rs `signed_in`): Claude Code's
# OAuth refresh token, not past its refreshTokenExpiresAt, or Codex's ChatGPT
# access and refresh tokens or API key, none of them blank (Claude Code blanks
# its tokens once a refresh is refused). Under auth_mode chatgptAuthTokens
# (tokens a host application supplies, which carry an empty refresh token) the
# access token alone counts: Codex 0.169.0 keeps those in memory and writes no
# such auth.json, but would load one. $Provider is the accounts folder's name
# for it, anthropic or openai.
function Test-AccountSignedIn([string]$Provider, [string]$Path) {
    $file = if ($Provider -eq 'anthropic') { '.credentials.json' } else { 'auth.json' }
    $json = try { Get-Content -LiteralPath (Join-Path $Path $file) -Raw -ErrorAction Stop | ConvertFrom-Json -AsHashtable } catch { $null }
    if (-not $json) { return $false }
    $text = { param($Value) $Value -is [string] -and $Value.Trim() -ne '' }
    if ($Provider -eq 'anthropic') {
        $oauth = $json.claudeAiOauth
        if (-not ($oauth -is [hashtable] -and (& $text $oauth.refreshToken))) { return $false }
        # An end that is no whole number of milliseconds is no end, as in Rust.
        $end = $oauth.refreshTokenExpiresAt
        return -not ($end -is [long] -or $end -is [int]) -or $end -lt 0 -or [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() -lt $end
    }
    $tokens = $json.tokens
    $external = $json.auth_mode -ceq 'chatgptAuthTokens'
    ($tokens -is [hashtable] -and (& $text $tokens.access_token) -and ($external -or (& $text $tokens.refresh_token))) -or (& $text $json.OPENAI_API_KEY)
}

# Whether $Path is a junction or symlink (any reparse point), not following it.
function Test-Link([string]$Path) {
    $item = Get-Item -LiteralPath $Path -Force -ErrorAction SilentlyContinue
    [bool]($item -and ($item.Attributes -band [IO.FileAttributes]::ReparsePoint))
}
