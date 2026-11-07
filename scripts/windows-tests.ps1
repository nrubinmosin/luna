#Requires -Version 7.4
# The crate's unit tests as Windows executables: built in the Windows toolchain
# image, run on a Windows host. The two halves run apart, on a Linux and a Windows
# machine in CI (.github/workflows/ci.yml), or one after the other here.
#
#   ./scripts/windows-tests.ps1 -Build      build them into test-build\windows-tests
#   ./scripts/windows-tests.ps1 -Run        run what is there
#   ./scripts/windows-tests.ps1 -Build -Run
#   -Dir <path>                             another folder (relative to the repo root)
#
# Which files are the tests is cargo's to say: its JSON messages name each test
# executable it built, so nothing here guesses by file name or time. -Build
# clears the folder's executables first, and -Run runs them from inside it, so a
# test that leans on the checkout or on the build tree fails here as in CI.
#
# -Build writes only into a folder directly in test-build/, and deletes nothing
# there but the executables it put there: -Dir is a path anyone may mistype, and
# a stray '.' must not cost the checkout. -Run only reads, so its -Dir may be
# anywhere, such as where CI downloads the build.
param(
    [switch]$Build,
    [switch]$Run,
    [string]$Dir = 'test-build/windows-tests'
)
. "$PSScriptRoot/lib.ps1"

if (-not ($Build -or $Run)) { throw 'say -Build, -Run or both' }
$Dir = [IO.Path]::GetFullPath($Dir, $RepoRoot)

# Throws unless $Path is a folder directly in test-build/ that -Build may fill,
# with no junction or symlink on the way there that could lead the deletes
# elsewhere.
function Assert-StagingDir([string]$Path) {
    $testBuild = [IO.Path]::GetFullPath('test-build', $RepoRoot)
    $parent = [IO.Path]::GetDirectoryName($Path.TrimEnd('\', '/'))
    $case = if ($IsWindows) { [StringComparison]::OrdinalIgnoreCase } else { [StringComparison]::Ordinal }
    if (-not [string]::Equals($parent, $testBuild, $case)) {
        throw "-Build stages into a folder directly in $testBuild, not $Path"
    }
    foreach ($p in $testBuild, $Path) {
        $item = Get-Item -LiteralPath $p -Force -ErrorAction SilentlyContinue
        if ($item -and ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw "-Build will not stage through a junction or symlink: $p"
        }
    }
}

if ($Build) {
    Assert-StagingDir $Dir
    $stamp = Get-BuildStamp

    # The crate embeds the frontend's dist/, which the tests never look at; an
    # empty one stands in when no build left a real one. stdout is cargo's JSON
    # alone; the human-readable diagnostics go to stderr. The messages file is
    # this run's own, and the build, the reading of it and the copies out of the
    # shared target dir are one step under the lock: another checkout's build
    # in between would have this one stage its executables.
    Use-BuildLock {
        $name = "windows-tests-$([guid]::NewGuid().ToString('n')).json"
        $messages = Join-Path $RepoRoot "test-build/$name"
        New-Item -ItemType Directory -Force (Split-Path $messages) | Out-Null
        try {
            $image = Build-Image luna-winbuild windows-build.Dockerfile
            Invoke-WinBuild 'Windows test build' $stamp $image @"
mkdir -p dist
cd src-tauri
cargo xwin test --locked --release --no-run --lib --bins --target x86_64-pc-windows-msvc --message-format=json-render-diagnostics > /app/test-build/$name
"@ $TestBuildEnv

            # Paths in the messages are the container's; /app is the repo root here.
            $exes = Get-Content -LiteralPath $messages | Where-Object { $_.StartsWith('{') } | ForEach-Object { $_ | ConvertFrom-Json } |
                Where-Object { $_.reason -eq 'compiler-artifact' -and $_.profile.test -and $_.executable } |
                ForEach-Object { Join-Path $RepoRoot ($_.executable -replace '^/app/', '') }
            if (-not $exes) { throw "cargo named no test executables in $messages" }

            New-Item -ItemType Directory -Force $Dir | Out-Null
            Get-ChildItem -LiteralPath $Dir -Filter *.exe -File -Force | ForEach-Object { Remove-Item -LiteralPath $_.FullName -Force }
            foreach ($exe in $exes) {
                Copy-Item -LiteralPath $exe -Destination $Dir
                Write-Host "test executable: $(Split-Path -Leaf $exe)"
            }
        } finally {
            Remove-Item -LiteralPath $messages -Force -ErrorAction SilentlyContinue
        }
    }
}

if ($Run) {
    $exes = @(Get-ChildItem -LiteralPath $Dir -Filter *.exe -File -ErrorAction SilentlyContinue)
    if (-not $exes) { throw "no test executables in $Dir - build them with -Build" }
    $passed = 0; $ignored = 0
    Push-Location -LiteralPath $Dir
    try {
        foreach ($exe in $exes) {
            Write-Host "== $($exe.Name)" -ForegroundColor Cyan
            # Echoed as it comes and kept for the counts below.
            & $exe.FullName 2>&1 | ForEach-Object { "$_" } | Tee-Object -Variable lines | Out-Host
            if ($LASTEXITCODE) { throw "$($exe.Name) failed (exit $LASTEXITCODE)" }
            foreach ($line in $lines) {
                if ($line -match '^test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored') {
                    $passed += [int]$Matches[1]; $ignored += [int]$Matches[3]
                }
            }
        }
    } finally {
        Pop-Location
    }
    Write-Host "Windows unit tests: $passed passed, $ignored ignored, in $($exes.Count) executables" -ForegroundColor Green
}
