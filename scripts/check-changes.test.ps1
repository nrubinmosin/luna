#Requires -Version 7.4
# Run the check against small Git histories of its own.
. "$PSScriptRoot/lib.ps1"

$failures = 0
function Check([string]$Name, [scriptblock]$Test) {
    try {
        & $Test
        Write-Host "ok   $Name"
    } catch {
        $script:failures++
        Write-Host "FAIL $($Name): $_" -ForegroundColor Red
    }
}

function Set-FixtureFile([string]$Path, [string]$Text) {
    $file = Join-Path $root $Path
    New-Item -ItemType Directory -Path (Split-Path -Parent $file) -Force | Out-Null
    Set-Content -LiteralPath $file $Text
}

function Invoke-FixtureGit {
    $gitArgs = $args
    Invoke-Checked "fixture git $($gitArgs[0])" {
        git -C $root -c user.name=Luna -c user.email=luna@example.invalid -c commit.gpgSign=false -c core.autocrlf=false @gitArgs
    }
}

function Initialize-Fixture {
    Invoke-FixtureGit init --quiet --initial-branch=master
    Set-FixtureFile 'CHANGELOG.md' "# Changelog`n`n## [Unreleased]`n`n## [1.0.0] - 2026-11-08"
    Set-FixtureFile 'changes/README.md' '# Instructions'
    Set-FixtureFile 'changes/existing.md' "### Fixed`n- An earlier fix."
    Set-FixtureFile 'src-tauri/src/main.rs' 'fn main() {}'
    Set-FixtureFile 'src-tauri/Cargo.toml' "[package]`nversion = '1.0.0'`n`n[dependencies]`na = '1.0'"
    Invoke-FixtureGit add --all
    Invoke-FixtureGit commit --quiet -m 'Create fixture'
    Invoke-FixtureGit update-ref refs/remotes/origin/master HEAD
    Invoke-FixtureGit switch --quiet --create slice/example
}

function Assert-Check([string]$Branch, [string]$Message = '', [string]$PullRequestBase) {
    $savedBase = $env:GITHUB_BASE_REF
    try {
        $env:GITHUB_BASE_REF = $PullRequestBase
        $output = pwsh -NoProfile -File "$PSScriptRoot/check-changes.ps1" -Root $root -Branch $Branch 2>&1
        $code = $LASTEXITCODE
    } finally {
        $env:GITHUB_BASE_REF = $savedBase
    }
    if ($Message) {
        if ($code -ne 1 -or -not ($output -join ' ').Contains($Message)) {
            throw "expected exit 1 and '$Message', got exit $code with $output"
        }
    } elseif ($code -ne 0) {
        throw "expected exit 0, got exit $code with $output"
    }
}

$work = Join-Path $RepoRoot "test-build/check-changes-tests/$([guid]::NewGuid().ToString('n'))"
try {
    $root = (New-Item -ItemType Directory -Path $work -Force).FullName
    Initialize-Fixture

    Check 'docs, scripts, fixtures and dedicated tests need no fragment' {
        Set-FixtureFile 'docs/guide.md' 'Guide.'
        Set-FixtureFile 'scripts/check.ps1' 'exit 0'
        Set-FixtureFile '.github/workflows/ci.yml' 'name: CI'
        Set-FixtureFile '.agents/skills/example/SKILL.md' 'Example.'
        Set-FixtureFile 'src-tauri/src/cli/testdata/example.txt' 'Example.'
        Set-FixtureFile 'src-tauri/crates/window/src/term/tests.rs' '// Test.'
        Set-FixtureFile 'src-tauri/crates/window/src/board_tests.rs' '// Test.'
        Set-FixtureFile 'src-tauri/crates/window/src/term/bench/mod.rs' '// Benchmark.'
        Assert-Check 'slice/example'
    }

    Check 'production dependency, feature and profile tables count in a modified manifest' {
        $manifest = 'src-tauri/Cargo.toml'
        $metadata = "[package]`nversion = '1.0.1'`n`n[dependencies]`n# Comment.`na = '1.0'`n`n[dev-dependencies]`nb = '2.0'"
        Set-FixtureFile $manifest $metadata
        Assert-Check 'slice/example'
        foreach ($table in @(
            "[target.'cfg(windows)'.dependencies]`nb = '2.0'"
            '[features]' + "`nnew = []"
            "[profile.release]`nlto = true"
        )) {
            Set-FixtureFile $manifest "$metadata`n`n$table"
            Assert-Check 'slice/example' $manifest
        }
        Invoke-FixtureGit restore -- $manifest
    }

    Check 'vendored source and a lockfile change each need a fragment' {
        foreach ($path in 'src-tauri/vendor/gpui-pre-windows/src/lib.rs', 'src-tauri/Cargo.lock') {
            Set-FixtureFile $path 'Changed.'
            try { Assert-Check 'slice/example' $path }
            finally { Remove-Item -LiteralPath (Join-Path $root $path) }
        }
    }

    Check 'the window and installer need a fragment and a new exemption satisfies it' {
        $window = 'src-tauri/crates/window/src/view.rs'
        $installer = 'src-tauri/windows/installer.nsi'
        Set-FixtureFile $window '// Window.'
        Assert-Check 'slice/example' $window
        Remove-Item -LiteralPath (Join-Path $root $window)
        Set-FixtureFile $installer '; Installer.'
        Assert-Check 'slice/example' $installer
        Set-FixtureFile 'changes/example.md' "### None`n`nInternal installer cleanup."
        Assert-Check 'slice/example'
        Remove-Item -LiteralPath (Join-Path $root $installer), (Join-Path $root 'changes/example.md')
    }

    Check 'a working tree and committed product change need a new fragment' {
        Set-FixtureFile 'src-tauri/src/main.rs' 'fn main() { println!("hello"); }'
        Set-FixtureFile 'changes/existing.md' "### Fixed`n- An edited entry."
        Assert-Check 'slice/example' '### None'
        Invoke-FixtureGit add src-tauri/src/main.rs changes/existing.md
        Invoke-FixtureGit commit --quiet -m 'Change the product'
        Assert-Check 'slice/example' 'require a new changes/<slug>.md'
        Set-FixtureFile 'changes/example.md' "### Changed`n- Luna greets you."
        Invoke-FixtureGit add changes/example.md
        Invoke-FixtureGit commit --quiet -m 'Record the change'
        Assert-Check 'slice/example'
    }

    Check 'master and release branches still validate content' {
        Remove-Item -LiteralPath (Join-Path $root 'changes/example.md')
        Assert-Check 'master'
        Assert-Check 'release/v1.1.0'
        Assert-Check 'release-1.1.0-rc.1'
        Assert-Check 'release-tooling' 'require a new changes/<slug>.md'
        Assert-Check 'release/v1.1' 'require a new changes/<slug>.md'
        Assert-Check 'release/v01.1.0' 'require a new changes/<slug>.md'
        Set-FixtureFile 'changes/example.md' "### Added`nMissing bullet."
        Assert-Check 'release/v1.1.0' 'changes/example.md'
        Remove-Item -LiteralPath (Join-Path $root 'changes/example.md')
        Set-FixtureFile 'CHANGELOG.md' "# Changelog`n`n## [Unreleased]`n`n### Added`n`n- Misplaced entry."
        Assert-Check 'master' 'keep ## [Unreleased] empty'
    }

    $root = (New-Item -ItemType Directory -Path (Join-Path $work 'bases')).FullName
    Initialize-Fixture
    Check 'pushes use the nearest integration merge base and exclude their own remote ref' {
        Set-FixtureFile 'src-tauri/src/main.rs' 'fn main() { println!("integration"); }'
        Invoke-FixtureGit add src-tauri/src/main.rs
        Invoke-FixtureGit commit --quiet -m 'Change the integration branch'
        Invoke-FixtureGit update-ref refs/remotes/origin/iteration/train HEAD
        Invoke-FixtureGit update-ref refs/remotes/origin/stage6/older HEAD^
        Set-FixtureFile 'docs/guide.md' 'A guide from the feature branch.'
        Invoke-FixtureGit add docs/guide.md
        Invoke-FixtureGit commit --quiet -m 'Add a guide'
        Assert-Check 'slice/example'
        Assert-Check 'slice/example' 'src-tauri/src/main.rs' -PullRequestBase 'master'
        Assert-Check 'slice/example' -PullRequestBase 'iteration/train'

        Set-FixtureFile 'src-tauri/src/main.rs' 'fn main() { println!("feature"); }'
        Invoke-FixtureGit add src-tauri/src/main.rs
        Invoke-FixtureGit commit --quiet -m 'Change the feature branch'
        Invoke-FixtureGit update-ref refs/remotes/origin/gpui/example HEAD
        Assert-Check 'gpui/example' 'src-tauri/src/main.rs'
    }
} finally {
    $resolved = (Resolve-Path -LiteralPath $work -ErrorAction SilentlyContinue).ProviderPath
    if ($resolved -and $resolved -eq $work) { Remove-Item -LiteralPath $resolved -Recurse -Force }
}

if ($failures) { exit 1 }
Write-Host 'check-changes tests passed' -ForegroundColor Green
