# What scripts/third-party-notices.ps1 decides, apart from the Docker runs that
# gather its input, so that scripts/notices-lib.test.ps1 can run it on small
# trees of its own: which crates the installer ships, each component's license
# texts, and the notices file made of them. Dot-source it after lib.ps1.

# The packages cargo metadata (--filter-platform for the shipped target) says
# are compiled into the root crate's binary: reached from the root through
# normal dependencies only. A build or dev dependency ships nothing; nor does a
# proc macro, which runs at compile time, or what it alone depends on. A
# member of the workspace (the native window's crate) is Luna's own code,
# under Luna's license: what it links ships, and it is not listed itself.
function Get-ShippedCrates($Metadata) {
    $members = @($Metadata.workspace_members)
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
            if ($members -notcontains $dep.pkg) { $shipped += $package }
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
# every component that has neither, unless it says why it has none (NoText:
# terms Microsoft publishes rather than a text): shipping it without its
# license's text would break that license. Returns the components with a Texts
# list added.
function Add-LicenseTexts([object[]]$Components) {
    $missing = @()
    $done = foreach ($component in $Components) {
        $texts = Read-LicenseTexts $component.Staged
        if (-not $texts) { $texts = Read-LicenseTexts $component.Supplied }
        if (-not $texts -and $component.PSObject.Properties['NoText'] -and $component.NoText) { $texts = @() }
        elseif (-not $texts) { $missing += "$($component.Name) $($component.Version)"; continue }
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
components below, compiled into luna.exe, bundled into its interface or part
of the installer itself, each under its own license. Each entry names its license and where its source is
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

# The Rust standard library's own inventory, COPYRIGHT-library.html from the
# toolchain's doc folder, as text: its in-tree licenses and copyright holders,
# and each out-of-tree crate std is built from with its notices. Tags go, the
# entities are decoded, and a <pre> block (a license text) keeps its lines.
function ConvertFrom-RustCopyright([string]$Html) {
    $html = $Html -replace "`r`n?", "`n"
    $html = [regex]::Replace($html, '(?s)<summary>\s*<code>(.*?)</code>\s*</summary>', { param($m) "`n--- $($m.Groups[1].Value)`n" })
    $html = [regex]::Replace($html, '(?s)<h(\d)[^>]*>(.*?)</h\1>', { param($m) "`n`n$($m.Groups[2].Value)`n" })
    $parts = [regex]::Split($html, '(?s)(<pre>.*?</pre>)')
    $text = -join $(foreach ($part in $parts) {
        if ($part.StartsWith('<pre>')) {
            $body = $part.Substring(5, $part.Length - 11)
            (([Net.WebUtility]::HtmlDecode($body) -split "`n" | ForEach-Object { $_.TrimEnd() }) -join "`n").Trim("`n") + "`n"
        } else {
            $plain = [Net.WebUtility]::HtmlDecode(($part -replace '(?s)<(head|style|script)\b.*?</\1>', '' -replace '<[^>]+>', ''))
            (($plain -split "`n" | ForEach-Object { $_.Trim() }) -join "`n")
        }
    })
    ($text -replace "`n{3,}", "`n`n").Trim()
}

# The license ids COPYRIGHT-library.html gives the in-tree files: their texts
# are not in it, only named, and come from the toolchain's licenses folder.
function Get-RustInTreeLicenses([string]$Html) {
    $at = $Html.IndexOf('id="out-of-tree-dependencies"')
    $inTree = if ($at -ge 0) { $Html.Substring(0, $at) } else { $Html }
    $ids = foreach ($m in [regex]::Matches($inTree, '<b>License:</b>\s*([^<]+)')) {
        $m.Groups[1].Value -split '[\s()]+' | Where-Object { $_ -and $_ -notin 'AND', 'OR', 'WITH' }
    }
    @($ids | Sort-Object -Unique -Culture ([cultureinfo]::InvariantCulture))
}

# Each crate tie holds an installer component to the version it was reviewed against.
function Assert-ComponentTies([object[]]$Components, [string]$CargoLock) {
    $problems = foreach ($component in $Components) {
        $tie = $component.tiedTo
        if (-not $tie) { continue }
        $name = [regex]::Escape($tie.crate)
        $locked = @([regex]::Matches($CargoLock, "(?m)^name = `"$name`"\r?\nversion = `"([^`"]+)`"") |
                ForEach-Object { $_.Groups[1].Value } | Sort-Object -Unique)
        if ($locked.Count -ne 1 -or $locked[0] -cne $tie.version) {
            "'$($component.name)' was checked against crate $($tie.crate) $($tie.version), and the lockfile has $(if ($locked) { $locked -join ', ' } else { 'none' }): check its version, license and texts again, then its tiedTo"
        }
    }
    if ($problems) { throw ($problems -join '; ') }
}

# An entry of components.json as a component for Add-LicenseTexts.
function ConvertTo-ComponentItem($Component, [string]$Supplied) {
    foreach ($field in 'name', 'version', 'license', 'source', 'note') {
        if (-not ($Component.$field -is [string] -and $Component.$field.Trim())) { throw "components.json has an entry without '$field'" }
    }
    if (-not $Component.texts -and -not $Component.noText) { throw "components.json's '$($Component.name)' names neither its texts nor why it has none (noText)" }
    [pscustomobject]@{
        Name     = $Component.name
        Version  = $Component.version
        License  = $Component.license
        Source   = $Component.source
        Note     = $Component.note + $(if ($Component.noText) { " $($Component.noText)" })
        Staged   = $null
        Supplied = if ($Component.texts) { Join-Path $Supplied $Component.texts }
        NoText   = [bool]$Component.noText
    }
}