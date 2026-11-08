#Requires -Version 7.4
# The fuzz targets in src-tauri/fuzz (docs/testing.md, "Fuzzing"), built with
# cargo-fuzz in the fuzz image (docker/fuzz.Dockerfile) and run there.
#
#   ./scripts/fuzz.ps1                              each target for 60 s
#   ./scripts/fuzz.ps1 -Target codex_rollout -Seconds 600
#   ./scripts/fuzz.ps1 -Target claude_transcript -Reproduce <file>
#                                                   run one input once: a crash again, with its stack
#   ./scripts/fuzz.ps1 -Minimize                    shrink each corpus to the inputs that add coverage
#
# Everything a run keeps is under test-build/fuzz/<target>/: corpus/, what it
# has found that adds coverage, seeded from fixtures/transcripts/; artifacts/,
# an input that crashed (crash-*), ran out of time (timeout-*) or memory
# (oom-*); and run.log. A run that finds one fails, naming the file.
#
# The checkout goes in read-only: the crate is built from a copy of src-tauri/
# in the container, whose Cargo.lock starts as the app's own. The build holds
# the machine's build lock (Use-BuildLock); the run, minutes long, does not.
param(
    [ValidateSet('claude_transcript', 'codex_rollout')]
    [string[]]$Target = @('claude_transcript', 'codex_rollout'),
    [ValidateRange(1, 86400)]
    [int]$Seconds = 60,
    [string]$Reproduce,
    [switch]$Minimize
)
. "$PSScriptRoot/lib.ps1"

# Each target's seeds, the fixtures of the provider it reads.
$Seeds = @{ claude_transcript = 'claude'; codex_rollout = 'codex' }

if ($Reproduce -and $Target.Count -ne 1) { throw '-Reproduce takes one -Target, the one that found it' }
if ($Reproduce -and $Minimize) { throw 'say -Reproduce or -Minimize, not both' }
if ($Reproduce) {
    $Reproduce = [IO.Path]::GetFullPath($Reproduce, (Get-Location).Path)
    if (-not (Test-Path -LiteralPath $Reproduce -PathType Leaf)) { throw "no such file: $Reproduce" }
}

# The nightly is pinned twice, in the image and beside the targets; a bump of
# one alone would have rustup fetch the other into every throwaway container.
$dockerfile = Get-Content (Join-Path $RepoRoot 'docker/fuzz.Dockerfile') -Raw
$toolchain = Get-Content (Join-Path $RepoRoot 'src-tauri/fuzz/rust-toolchain.toml') -Raw
$inImage = [regex]::Match($dockerfile, 'rustup toolchain install (nightly-\d{4}-\d{2}-\d{2})').Groups[1].Value
$beside = [regex]::Match($toolchain, '(?m)^channel\s*=\s*"(nightly-\d{4}-\d{2}-\d{2})"').Groups[1].Value
if (-not $inImage -or $inImage -ne $beside) {
    throw "the fuzz toolchain pins disagree: docker/fuzz.Dockerfile installs '$inImage', src-tauri/fuzz/rust-toolchain.toml names '$beside'"
}

$out = Join-Path $RepoRoot 'test-build/fuzz'
foreach ($t in $Target) {
    foreach ($d in 'corpus', 'artifacts') { New-Item -ItemType Directory -Force (Join-Path $out "$t/$d") | Out-Null }
}

# The volumes the fuzz image builds into; every checkout shares them under the
# same paths, hence the lock inside the target volume, as Invoke-LinuxCargo
# keeps one for its own.
$volumes = @('-v', 'luna-fuzz-target:/target', '-v', 'luna-linux-cargo-registry:/usr/local/cargo/registry')
$bin = '/target/x86_64-unknown-linux-gnu/release'

$stamp = Get-BuildStamp
# Set inside the lock's script block, which runs in a scope of its own.
$images = @{}
Use-BuildLock {
    $images.fuzz = Build-Image luna-fuzz fuzz.Dockerfile
    # Both targets, whichever is run: cargo fuzz builds one or all. Copied, so
    # the build writes nothing into the checkout (tauri-build's
    # gen/, the fuzz crate's Cargo.lock); the copy's files are all new, so cargo
    # rebuilds the crate from them whatever another checkout built last.
    $script = ConvertTo-BashScript @"
set -e
exec 9>/target/.fuzz.lock
flock -n 9 || { echo 'another fuzz build is using these volumes; waiting for it'; flock 9; }
mkdir -p /work
tar -C /app --exclude=src-tauri/target --exclude=src-tauri/gen --exclude=src-tauri/fuzz/target -cf - src-tauri | tar -C /work -xf -
cd /work/src-tauri/fuzz
cp ../Cargo.lock Cargo.lock
cargo fuzz build --sanitizer none --debug-assertions --target-dir /target
"@
    Invoke-Checked 'cargo fuzz build' {
        docker run --rm -e LUNA_BUILD=$stamp -e LUNA_FLAVOR=test -v "${RepoRoot}:/app:ro" @volumes $images.fuzz bash -c $script
    }
}

# libFuzzer's own options: a unit gets 10 s and 2 GB, and up to 256 KiB of
# input, a few transcript lines of the size Claude Code writes after a turn.
$limits = '-timeout=10 -rss_limit_mb=2048 -max_len=262144'
$failed = @()
foreach ($t in $Target) {
    $seeds = "/app/src-tauri/fixtures/transcripts/$($Seeds[$t])"
    $dict = "/app/src-tauri/fuzz/$t.dict"
    $mounts = @('-v', "${RepoRoot}:/app:ro", '-v', "$(Join-Path $out $t):/out") + $volumes
    if ($Reproduce) {
        $mounts += @('-v', "$(Split-Path $Reproduce):/repro:ro")
        $run = "$bin/$t $limits -runs=1 '/repro/$(Split-Path -Leaf $Reproduce)'"
    } elseif ($Minimize) {
        # Into a new folder, then in place of the old: a merge that fails
        # leaves the corpus as it was.
        $run = @"
rm -rf /out/corpus.min && mkdir /out/corpus.min
$bin/$t $limits -dict=$dict -merge=1 /out/corpus.min /out/corpus $seeds
rm -rf /out/corpus && mv /out/corpus.min /out/corpus
"@
    } else {
        $run = "$bin/$t $limits -dict=$dict -max_total_time=$Seconds -print_final_stats=1 -artifact_prefix=/out/artifacts/ /out/corpus $seeds 2>&1 | tee /out/run.log; exit `${PIPESTATUS[0]}"
    }
    Write-Host "== $t"
    docker run --rm @mounts $images.fuzz bash -c (ConvertTo-BashScript "set -e`n$run")
    if ($LASTEXITCODE) { $failed += $t }
}

if ($failed) {
    $found = foreach ($t in $failed) { Get-ChildItem (Join-Path $out "$t/artifacts") -File | ForEach-Object FullName }
    throw "fuzzing failed for $($failed -join ', '); see test-build/fuzz/<target>/run.log$(if ($found) { "; inputs: $($found -join ', ')" })"
}
