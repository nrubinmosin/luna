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
# empties the folder first, and -Run runs the executables from inside it, so a
# test that leans on the checkout or on the build tree fails here as in CI.
param(
    [switch]$Build,
    [switch]$Run,
    [string]$Dir = 'test-build/windows-tests'
)
. "$PSScriptRoot/lib.ps1"

if (-not ($Build -or $Run)) { throw 'say -Build, -Run or both' }
$Dir = [IO.Path]::GetFullPath($Dir, $RepoRoot)

if ($Build) {
    $stamp = Get-BuildStamp
    $messages = Join-Path $RepoRoot 'test-build/windows-tests.json'
    New-Item -ItemType Directory -Force (Split-Path $messages) | Out-Null
    Remove-Item -Force $messages -ErrorAction SilentlyContinue

    # The crate embeds the frontend's dist/, which the tests never look at; an
    # empty one stands in when no build left a real one. stdout is cargo's JSON
    # alone; the human-readable diagnostics go to stderr.
    Use-BuildLock {
        $image = Build-Image luna-winbuild windows-build.Dockerfile
        Invoke-WinBuild 'Windows test build' $stamp $image @'
mkdir -p dist
cd src-tauri
cargo xwin test --locked --release --no-run --lib --bins --target x86_64-pc-windows-msvc --message-format=json-render-diagnostics > /app/test-build/windows-tests.json
'@ $TestBuildEnv
    }

    # Paths in the messages are the container's; /app is the repo root here.
    $exes = Get-Content $messages | Where-Object { $_.StartsWith('{') } | ForEach-Object { $_ | ConvertFrom-Json } |
        Where-Object { $_.reason -eq 'compiler-artifact' -and $_.profile.test -and $_.executable } |
        ForEach-Object { Join-Path $RepoRoot ($_.executable -replace '^/app/', '') }
    if (-not $exes) { throw "cargo named no test executables in $messages" }

    if (Test-Path $Dir) { Remove-Item -Recurse -Force $Dir }
    New-Item -ItemType Directory -Force $Dir | Out-Null
    foreach ($exe in $exes) {
        Copy-Item $exe $Dir
        Write-Host "test executable: $(Split-Path -Leaf $exe)"
    }
}

if ($Run) {
    $exes = @(Get-ChildItem $Dir -Filter *.exe -ErrorAction SilentlyContinue)
    if (-not $exes) { throw "no test executables in $Dir - build them with -Build" }
    $passed = 0; $ignored = 0
    Push-Location $Dir
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
