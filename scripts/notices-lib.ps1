# What scripts/third-party-notices.ps1 decides, apart from the Docker runs that
# gather its input, so that scripts/notices-lib.test.ps1 can run it on small
# trees of its own: which crates the installer ships, each component's license
# texts, and the notices file made of them. Dot-source it after lib.ps1.

# The packages cargo metadata (--filter-platform for the shipped target) says
# are compiled into the root crate's binary: reached from the root through
# normal dependencies only. A build or dev dependency ships nothing; nor does a
# proc macro, which runs at compile time, or what it alone depends on.
function Get-ShippedCrates($Metadata) {
    $packages = @{}
    foreach ($package in $Metadata.packages) { $packages[$package.id] = $package }
    $nodes = @{}
    foreach ($node in $Metadata.resolve.nodes) { $nodes[$node.id] = $node }
    $root = $Metadata.resolve.root
    if (-not $root -or -not $nodes[$root]) { throw 'cargo metadata has no resolved root package' }

    $shipped = @()
    $queue = [Collections.Generic.Queue[string]]::new()
    $queue.Enqueue($root)
    $seen = @{ $root = $true }
    while ($queue.Count) {
        foreach ($dep in $nodes[$queue.Dequeue()].deps) {
            $normal = @($dep.dep_kinds | Where-Object { $null -eq $_.kind }).Count -gt 0
            if (-not $normal -or $seen[$dep.pkg]) { continue }
            $seen[$dep.pkg] = $true
            $package = $packages[$dep.pkg]
            if ($package.targets | Where-Object { $_.kind -contains 'proc-macro' }) { continue }
            $shipped += $package
            $queue.Enqueue($dep.pkg)
        }
    }
    @($shipped | Sort-Object name, version -Culture ([cultureinfo]::InvariantCulture))
}

# The license texts in a folder, by their path in it, with line endings and
# trailing space made uniform so the notices come out the same on any host.
# An SPDX identifier file (`LICENSE.spdx`) names a license but is not its text.
function Read-LicenseTexts([string]$Folder) {
    if (-not $Folder -or -not (Test-Path -LiteralPath $Folder -PathType Container)) { return @() }
    $root = (Resolve-Path -LiteralPath $Folder).Path
    $files = Get-ChildItem -LiteralPath $root -File -Recurse | Where-Object { $_.Extension -ne '.spdx' } |
        ForEach-Object { [pscustomobject]@{ Name = $_.FullName.Substring($root.Length + 1).Replace('\', '/'); Path = $_.FullName } } |
        Sort-Object Name -Culture ([cultureinfo]::InvariantCulture)
    @(foreach ($file in $files) {
        $text = (((Get-Content -LiteralPath $file.Path -Raw) -replace "`r`n?", "`n") -split "`n" | ForEach-Object { $_.TrimEnd() }) -join "`n"
        if ($text.Trim()) { [pscustomobject]@{ Name = $file.Name; Text = $text.Trim("`n") } }
    })
}

# Each component's texts: those gathered from its own package ($_.Staged), else
# those this repo keeps for it ($_.Supplied, third-party-licenses/). Throws for
# every component that has neither: shipping it without its license's text
# would break that license. Returns the components with a Texts list added.
function Add-LicenseTexts([object[]]$Components) {
    $missing = @()
    $done = foreach ($component in $Components) {
        $texts = Read-LicenseTexts $component.Staged
        if (-not $texts) { $texts = Read-LicenseTexts $component.Supplied }
        if (-not $texts) { $missing += "$($component.Name) $($component.Version)"; continue }
        $component | Add-Member -NotePropertyName Texts -NotePropertyValue $texts -PassThru -Force
    }
    if ($missing) {
        throw "no license text for $($missing -join ', '): the package publishes none, so add it under third-party-licenses/ from the package's repository (see its README)"
    }
    @($done)
}

# The notices file: a header, then each component with its license, where its
# source is, and its license texts. A text several components share word for
# word is written once, at the end, and referred to by number.
function New-NoticesText([string]$Version, [object[]]$Sections) {
    $count = @{}
    foreach ($item in $Sections.Items) {
        foreach ($text in $item.Texts) { $count[$text.Text] = 1 + [int]$count[$text.Text] }
    }
    $shared = [ordered]@{}
    $rule = '=' * 78
    $out = [Text.StringBuilder]::new()
    [void]$out.Append([string]((@"
Third-party notices for Luna $Version

Luna is licensed under MIT OR Apache-2.0. Its installer also carries the
components below, compiled into luna.exe or bundled into its interface, each
under its own license. Each entry names its license and where its source is
available, then gives the license texts it is published with; a text several
entries share word for word is given once, at the end.
"@) -replace "`r`n", "`n"))
    foreach ($section in $Sections) {
        [void]$out.Append("`n`n$rule`n$($section.Title) ($(@($section.Items).Count))`n$rule`n")
        foreach ($item in $section.Items) {
            [void]$out.Append("`n$($item.Name) $($item.Version)`nLicense: $($item.License)`nSource: $($item.Source)`n")
            if ($item.Note) { [void]$out.Append("$($item.Note)`n") }
            foreach ($text in $item.Texts) {
                if ($count[$text.Text] -gt 1) {
                    if (-not $shared.Contains($text.Text)) { $shared[$text.Text] = $shared.Count + 1 }
                    [void]$out.Append("$($text.Name): shared text $($shared[$text.Text])`n")
                } else {
                    [void]$out.Append("`n--- $($text.Name)`n$($text.Text)`n")
                }
            }
        }
    }
    if ($shared.Count) {
        [void]$out.Append("`n`n$rule`nShared license texts`n$rule`n")
        foreach ($entry in $shared.GetEnumerator()) {
            [void]$out.Append("`n--- shared text $($entry.Value)`n$($entry.Key)`n")
        }
    }
    $out.ToString()
}

# The statement MPL-2.0's section 3.2 asks of a binary distribution: where the
# recipient can get the source of the covered code.
function Get-LicenseNote([string]$License) {
    if ($License -match '\bMPL-2\.0\b') {
        'This is Covered Software under the Mozilla Public License 2.0; its Source Code Form is available, unmodified, at the Source above.'
    }
}
