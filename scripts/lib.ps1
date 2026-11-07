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
function Invoke-Checked([string]$What, [scriptblock]$Command) {
    & $Command
    if ($LASTEXITCODE) { throw "$What failed (exit $LASTEXITCODE)" }
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
