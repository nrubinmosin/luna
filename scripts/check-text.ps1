#Requires -Version 7.4
# Keeps tracked text in US English. Each finding is printed as path:line:col
# message. Known text types must also be UTF-8, and links and directories
# are skipped.
#
# A comment ending in "lang-check: allow" skips its line. When the comment
# stands alone, it also skips the next line. The comment opener must start
# the line or follow whitespace.
#
#   pwsh -File scripts/check-text.ps1           check the files Git tracks
#   pwsh -File scripts/check-text.ps1 FILE...   check just these files

$ErrorActionPreference = 'Stop'
$textCheckRoot = [IO.Path]::GetDirectoryName($PSScriptRoot)

# Match JavaScript whitespace, which includes BOM and excludes NEL.
$space = '[\x09-\x0d\x20\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000\ufeff]'
$marker = 'lang-check: allow'
$markerTail = "$space*$marker$space*(?:-->|\*/)?$space*\z"
$markerEnd = [regex]"(?:^$space*\*|(?:^|$space)(?://+|#|<!--|/\*+|--))$markerTail"
$markerLine = [regex]"^$space*(?://+|#|<!--|/\*+|\*|--)$markerTail"

# Leave generated files and third-party text as published. The checker and
# its tests contain the words and letters the check looks for.
$skipPaths = [regex](@(
    '(^|/)pnpm-lock\.yaml\z',
    '^patches/',
    '(^|/)Cargo\.lock\z',
    '^src-tauri/gen/',
    '^src-tauri/vendor/[^/]+/',
    '^third-party-licenses/(crates|npm|tools|installer)/',
    '^scripts/check-text(?:\.test)?\.ps1\z'
) -join '|')

$binaryTypes = '.png .ico .icns .jpg .jpeg .gif .webp .bmp .pdf .woff .woff2 .ttf .otf .zip .gz .exe .dll .wasm' -split ' '
$textTypes = @'
.rs .ts .tsx .js .mjs .cjs .json .md .css .html .svg .xml .toml .yaml .yml .txt .ps1 .sh .nsh
.dockerfile .editorconfig .gitattributes .gitignore .prettierignore
'@ -split '\s+'

# Unicode 17 ranges for Letter excluding Script=Latin, from the former
# Node checker's /(?=\p{L})\P{Script=Latin}/u. .NET does not expose Unicode
# scripts, and its category tables vary with the PowerShell runtime.
$nonLatinRanges = @'
B5 2B9-2C1 2C6-2D1 2EC 2EE 370-374 376-377 37A-37D 37F 386 388-38A 38C 38E-3A1 3A3-3F5
3F7-481 48A-52F 531-556 559 560-588 5D0-5EA 5EF-5F2 620-64A 66E-66F 671-6D3 6D5 6E5-6E6
6EE-6EF 6FA-6FC 6FF 710 712-72F 74D-7A5 7B1 7CA-7EA 7F4-7F5 7FA 800-815 81A 824 828
840-858 860-86A 870-887 889-88F 8A0-8C9 904-939 93D 950 958-961 971-980 985-98C 98F-990
993-9A8 9AA-9B0 9B2 9B6-9B9 9BD 9CE 9DC-9DD 9DF-9E1 9F0-9F1 9FC A05-A0A A0F-A10 A13-A28
A2A-A30 A32-A33 A35-A36 A38-A39 A59-A5C A5E A72-A74 A85-A8D A8F-A91 A93-AA8 AAA-AB0
AB2-AB3 AB5-AB9 ABD AD0 AE0-AE1 AF9 B05-B0C B0F-B10 B13-B28 B2A-B30 B32-B33 B35-B39 B3D
B5C-B5D B5F-B61 B71 B83 B85-B8A B8E-B90 B92-B95 B99-B9A B9C B9E-B9F BA3-BA4 BA8-BAA
BAE-BB9 BD0 C05-C0C C0E-C10 C12-C28 C2A-C39 C3D C58-C5A C5C-C5D C60-C61 C80 C85-C8C
C8E-C90 C92-CA8 CAA-CB3 CB5-CB9 CBD CDC-CDE CE0-CE1 CF1-CF2 D04-D0C D0E-D10 D12-D3A D3D
D4E D54-D56 D5F-D61 D7A-D7F D85-D96 D9A-DB1 DB3-DBB DBD DC0-DC6 E01-E30 E32-E33 E40-E46
E81-E82 E84 E86-E8A E8C-EA3 EA5 EA7-EB0 EB2-EB3 EBD EC0-EC4 EC6 EDC-EDF F00 F40-F47
F49-F6C F88-F8C 1000-102A 103F 1050-1055 105A-105D 1061 1065-1066 106E-1070 1075-1081 108E
10A0-10C5 10C7 10CD 10D0-10FA 10FC-1248 124A-124D 1250-1256 1258 125A-125D 1260-1288
128A-128D 1290-12B0 12B2-12B5 12B8-12BE 12C0 12C2-12C5 12C8-12D6 12D8-1310 1312-1315
1318-135A 1380-138F 13A0-13F5 13F8-13FD 1401-166C 166F-167F 1681-169A 16A0-16EA 16F1-16F8
1700-1711 171F-1731 1740-1751 1760-176C 176E-1770 1780-17B3 17D7 17DC 1820-1878 1880-1884
1887-18A8 18AA 18B0-18F5 1900-191E 1950-196D 1970-1974 1980-19AB 19B0-19C9 1A00-1A16
1A20-1A54 1AA7 1B05-1B33 1B45-1B4C 1B83-1BA0 1BAE-1BAF 1BBA-1BE5 1C00-1C23 1C4D-1C4F
1C5A-1C7D 1C80-1C8A 1C90-1CBA 1CBD-1CBF 1CE9-1CEC 1CEE-1CF3 1CF5-1CF6 1CFA 1D26-1D2B
1D5D-1D61 1D66-1D6A 1D78 1DBF 1F00-1F15 1F18-1F1D 1F20-1F45 1F48-1F4D 1F50-1F57 1F59 1F5B
1F5D 1F5F-1F7D 1F80-1FB4 1FB6-1FBC 1FBE 1FC2-1FC4 1FC6-1FCC 1FD0-1FD3 1FD6-1FDB 1FE0-1FEC
1FF2-1FF4 1FF6-1FFC 2102 2107 210A-2113 2115 2119-211D 2124 2126 2128 212C-212D 212F-2131
2133-2139 213C-213F 2145-2149 2C00-2C5F 2C80-2CE4 2CEB-2CEE 2CF2-2CF3 2D00-2D25 2D27 2D2D
2D30-2D67 2D6F 2D80-2D96 2DA0-2DA6 2DA8-2DAE 2DB0-2DB6 2DB8-2DBE 2DC0-2DC6 2DC8-2DCE
2DD0-2DD6 2DD8-2DDE 2E2F 3005-3006 3031-3035 303B-303C 3041-3096 309D-309F 30A1-30FA
30FC-30FF 3105-312F 3131-318E 31A0-31BF 31F0-31FF 3400-4DBF 4E00-A48C A4D0-A4FD A500-A60C
A610-A61F A62A-A62B A640-A66E A67F-A69D A6A0-A6E5 A717-A71F A788 A800-A801 A803-A805
A807-A80A A80C-A822 A840-A873 A882-A8B3 A8F2-A8F7 A8FB A8FD-A8FE A90A-A925 A930-A946
A960-A97C A984-A9B2 A9CF A9E0-A9E4 A9E6-A9EF A9FA-A9FE AA00-AA28 AA40-AA42 AA44-AA4B
AA60-AA76 AA7A AA7E-AAAF AAB1 AAB5-AAB6 AAB9-AABD AAC0 AAC2 AADB-AADD AAE0-AAEA AAF2-AAF4
AB01-AB06 AB09-AB0E AB11-AB16 AB20-AB26 AB28-AB2E AB65 AB70-ABE2 AC00-D7A3 D7B0-D7C6
D7CB-D7FB F900-FA6D FA70-FAD9 FB13-FB17 FB1D FB1F-FB28 FB2A-FB36 FB38-FB3C FB3E FB40-FB41
FB43-FB44 FB46-FBB1 FBD3-FD3D FD50-FD8F FD92-FDC7 FDF0-FDFB FE70-FE74 FE76-FEFC FF66-FFBE
FFC2-FFC7 FFCA-FFCF FFD2-FFD7 FFDA-FFDC 10000-1000B 1000D-10026 10028-1003A 1003C-1003D
1003F-1004D 10050-1005D 10080-100FA 10280-1029C 102A0-102D0 10300-1031F 1032D-10340
10342-10349 10350-10375 10380-1039D 103A0-103C3 103C8-103CF 10400-1049D 104B0-104D3
104D8-104FB 10500-10527 10530-10563 10570-1057A 1057C-1058A 1058C-10592 10594-10595
10597-105A1 105A3-105B1 105B3-105B9 105BB-105BC 105C0-105F3 10600-10736 10740-10755
10760-10767 10800-10805 10808 1080A-10835 10837-10838 1083C 1083F-10855 10860-10876
10880-1089E 108E0-108F2 108F4-108F5 10900-10915 10920-10939 10940-10959 10980-109B7
109BE-109BF 10A00 10A10-10A13 10A15-10A17 10A19-10A35 10A60-10A7C 10A80-10A9C 10AC0-10AC7
10AC9-10AE4 10B00-10B35 10B40-10B55 10B60-10B72 10B80-10B91 10C00-10C48 10C80-10CB2
10CC0-10CF2 10D00-10D23 10D4A-10D65 10D6F-10D85 10E80-10EA9 10EB0-10EB1 10EC2-10EC7
10F00-10F1C 10F27 10F30-10F45 10F70-10F81 10FB0-10FC4 10FE0-10FF6 11003-11037 11071-11072
11075 11083-110AF 110D0-110E8 11103-11126 11144 11147 11150-11172 11176 11183-111B2
111C1-111C4 111DA 111DC 11200-11211 11213-1122B 1123F-11240 11280-11286 11288 1128A-1128D
1128F-1129D 1129F-112A8 112B0-112DE 11305-1130C 1130F-11310 11313-11328 1132A-11330
11332-11333 11335-11339 1133D 11350 1135D-11361 11380-11389 1138B 1138E 11390-113B5 113B7
113D1 113D3 11400-11434 11447-1144A 1145F-11461 11480-114AF 114C4-114C5 114C7 11580-115AE
115D8-115DB 11600-1162F 11644 11680-116AA 116B8 11700-1171A 11740-11746 11800-1182B
118A0-118DF 118FF-11906 11909 1190C-11913 11915-11916 11918-1192F 1193F 11941 119A0-119A7
119AA-119D0 119E1 119E3 11A00 11A0B-11A32 11A3A 11A50 11A5C-11A89 11A9D 11AB0-11AF8
11BC0-11BE0 11C00-11C08 11C0A-11C2E 11C40 11C72-11C8F 11D00-11D06 11D08-11D09 11D0B-11D30
11D46 11D60-11D65 11D67-11D68 11D6A-11D89 11D98 11DB0-11DDB 11EE0-11EF2 11F02 11F04-11F10
11F12-11F33 11FB0 12000-12399 12480-12543 12F90-12FF0 13000-1342F 13441-13446 13460-143FA
14400-14646 16100-1611D 16800-16A38 16A40-16A5E 16A70-16ABE 16AD0-16AED 16B00-16B2F
16B40-16B43 16B63-16B77 16B7D-16B8F 16D40-16D6C 16E40-16E7F 16EA0-16EB8 16EBB-16ED3
16F00-16F4A 16F50 16F93-16F9F 16FE0-16FE1 16FE3 16FF2-16FF3 17000-18CD5 18CFF-18D1E
18D80-18DF2 1AFF0-1AFF3 1AFF5-1AFFB 1AFFD-1AFFE 1B000-1B122 1B132 1B150-1B152 1B155
1B164-1B167 1B170-1B2FB 1BC00-1BC6A 1BC70-1BC7C 1BC80-1BC88 1BC90-1BC99 1D400-1D454
1D456-1D49C 1D49E-1D49F 1D4A2 1D4A5-1D4A6 1D4A9-1D4AC 1D4AE-1D4B9 1D4BB 1D4BD-1D4C3
1D4C5-1D505 1D507-1D50A 1D50D-1D514 1D516-1D51C 1D51E-1D539 1D53B-1D53E 1D540-1D544 1D546
1D54A-1D550 1D552-1D6A5 1D6A8-1D6C0 1D6C2-1D6DA 1D6DC-1D6FA 1D6FC-1D714 1D716-1D734
1D736-1D74E 1D750-1D76E 1D770-1D788 1D78A-1D7A8 1D7AA-1D7C2 1D7C4-1D7CB 1E030-1E06D
1E100-1E12C 1E137-1E13D 1E14E 1E290-1E2AD 1E2C0-1E2EB 1E4D0-1E4EB 1E5D0-1E5ED 1E5F0
1E6C0-1E6DE 1E6E0-1E6E2 1E6E4-1E6E5 1E6E7-1E6ED 1E6F0-1E6F4 1E6FE-1E6FF 1E7E0-1E7E6
1E7E8-1E7EB 1E7ED-1E7EE 1E7F0-1E7FE 1E800-1E8C4 1E900-1E943 1E94B 1EE00-1EE03 1EE05-1EE1F
1EE21-1EE22 1EE24 1EE27 1EE29-1EE32 1EE34-1EE37 1EE39 1EE3B 1EE42 1EE47 1EE49 1EE4B
1EE4D-1EE4F 1EE51-1EE52 1EE54 1EE57 1EE59 1EE5B 1EE5D 1EE5F 1EE61-1EE62 1EE64 1EE67-1EE6A
1EE6C-1EE72 1EE74-1EE77 1EE79-1EE7C 1EE7E 1EE80-1EE89 1EE8B-1EE9B 1EEA1-1EEA3 1EEA5-1EEA9
1EEAB-1EEBB 20000-2A6DF 2A700-2B81D 2B820-2CEAD 2CEB0-2EBE0 2EBF0-2EE5D 2F800-2FA1D
30000-3134A 31350-33479
'@ -split '\s+'

# Regex matches UTF-16, so split supplementary ranges at each high surrogate.
# Match the pair as one letter and keep the original UTF-16 column numbers.
$bmp = [Text.StringBuilder]::new()
$surrogates = [ordered]@{}
foreach ($range in $nonLatinRanges) {
    $firstHex, $lastHex = $range -split '-'
    $first = [Convert]::ToInt32($firstHex, 16)
    $last = if ($lastHex) { [Convert]::ToInt32($lastHex, 16) } else { $first }
    if ($first -le 0xffff) {
        [void]$bmp.Append(('\u{0:x4}' -f $first))
        if ($last -ne $first) { [void]$bmp.Append(('-\u{0:x4}' -f $last)) }
        continue
    }
    for ($code = $first - 0x10000; $code -le $last - 0x10000; $code = $end + 1) {
        $high = 0xd800 + ($code -shr 10)
        $end = [Math]::Min($last - 0x10000, ($code -bor 0x3ff))
        if (-not $surrogates.Contains([string]$high)) {
            $surrogates[[string]$high] = [Text.StringBuilder]::new()
        }
        $low = $surrogates[[string]$high]
        [void]$low.Append(('\u{0:x4}' -f (0xdc00 + ($code -band 0x3ff))))
        if ($end -ne $code) { [void]$low.Append(('-\u{0:x4}' -f (0xdc00 + ($end -band 0x3ff)))) }
    }
}
$letterPatterns = @("[$bmp]"; foreach ($pair in $surrogates.GetEnumerator()) {
        '\u{0:x4}[{1}]' -f [int]$pair.Key, $pair.Value
    })
$nonLatinLetter = [regex]::new(($letterPatterns -join '|'), 'Compiled')

# British spellings and the American forms to suggest. Each pattern matches
# a whole word with its inflections.
$british = [ordered]@{
    '(?:re|dis)?colour(?:s|ed|ing|ful|less)?' = 'color'
    'behaviours?|behavioural(?:ly)?' = 'behavior'
    'flavours?|flavoured' = 'flavor'
    'favours?|favoured|favouring|favourites?|favourable' = 'favor'
    'honours?|honoured|honouring' = 'honor'
    'neighbours?|neighbouring' = 'neighbor'
    'labours?|laboured|labouring' = 'labor'
    'humours?|rumours?|odours?|vapours?|armours?|harbours?|endeavours?' = 'humor, rumor, …'
    'licences?' = 'license'
    'defences?|offences?|pretences?' = 'defense, offense, pretense'
    'practis(?:e|ed|es|ing)' = 'practice'
    'centres?|centred|centring' = 'center'
    'metres?|litres?|fibres?|theatres?|calibres?' = 'meter, liter, …'
    'catalogues?|catalogued|cataloguing' = 'catalog'
    'analogues?' = 'analog'
    'greys?|greyed|greying|greyish' = 'gray'
    'analys(?:e|ed|ing)|paralys(?:e|ed|es|ing)' = 'analyze'
    'judgements?' = 'judgment'
    'acknowledgements?' = 'acknowledgment'
    'ageing' = 'aging'
    'artefacts?' = 'artifact'
    'cheques?' = 'check'
    'tyres?' = 'tire'
    'programmes?' = 'program'
    'manoeuvres?' = 'maneuver'
    'sceptic(?:al|s)?' = 'skeptic'
    'whilst' = 'while'
    'amongst' = 'among'
    'learnt' = 'learned'
    'spelt' = 'spelled'
    'focuss(?:ed|es|ing)' = 'focused'
    'benefitt(?:ed|ing)' = 'benefited'
    'enrolments?|fulfilment|instalments?' = 'enrollment, fulfillment, installment'
    '(?:skil|wil)ful(?:ly)?' = 'skillful, willful'
    'jewellery' = 'jewelry'
    'orientated' = 'oriented'
    'fortnights?' = 'two weeks'
    # The doubled l before -ed/-ing/-er where American English keeps one.
    '(?:cancel|travel|model|label|signal|level|fuel|channel|total|marshal|tunnel|funnel|counsel|dial|pedal|portal|quarrel)l(?:ed|ing|ers?)' = 'a single l (canceled, modeling)'
}

# The listed stems take -ize, including their inflected forms.
$iseStems = @'
apologis authoris canonicalis capitalis categoris centralis characteris criticis customis deserialis
digitis finalis formalis generalis globalis initialis italicis localis materialis maximis memoris
minimis modernis normalis optimis organis parameteris personalis prioritis randomis realis recognis
regularis reinitialis reorganis sanitis serialis specialis stabilis standardis summaris symbolis
synchronis tokenis unrecognis utilis virtualis visualis
'@ -split '\s+'
foreach ($stem in $iseStems) {
    $british["$stem(?:e|ed|es|ing|ers?|ations?|able)"] = $stem.Substring(0, $stem.Length - 1) + 'z…'
}
# Emphasis is already American as a noun.
$british['emphasis(?:e|ed|es|ing)'] = 'emphasize'

# JavaScript's Unicode case-insensitive word boundary is ASCII after case
# folding, so spell it out instead of using .NET's Unicode \b.
$suggestions = @($british.Values)
$spellings = for ($i = 0; $i -lt $british.Count; $i++) {
    '(?<rule{0}>{1})' -f $i, @($british.Keys)[$i]
}
$britishWord = [regex]::new(('(?<![a-z0-9_])(?:{0})(?![a-z0-9_])' -f ($spellings -join '|')), 'Compiled')
$utf8 = [Text.UTF8Encoding]::new($false, $true)

function Get-RepoTextPath([string]$Path) {
    $cwd = (Get-Location -PSProvider FileSystem).ProviderPath
    if (-not $Path) { $Path = '.' }
    $absolute = [IO.Path]::GetFullPath($Path.Replace('\', '/'), $cwd)
    [IO.Path]::GetRelativePath($textCheckRoot, $absolute).Replace('\', '/')
}

function Find-TextIssues([string]$Path, [string]$Content) {
    $lines = $Content -csplit '\r?\n'
    $previousMarker = $false
    for ($i = 0; $i -lt $lines.Count; $i++) {
        $line = $lines[$i]
        $skip = $previousMarker -or $markerEnd.IsMatch($line)
        $previousMarker = $markerLine.IsMatch($line)
        if ($skip) { continue }

        $letters = $nonLatinLetter.Matches($line)
        # Long s and the Kelvin sign also fold to ASCII in Unicode matching.
        $folded = $line.ToLowerInvariant().Replace([char]0x017f, [char]'s')
        $words = $britishWord.Matches($folded)
        $letterIndex = $wordIndex = 0
        while ($letterIndex -lt $letters.Count -or $wordIndex -lt $words.Count) {
            if ($letterIndex -lt $letters.Count -and
                ($wordIndex -ge $words.Count -or $letters[$letterIndex].Index -le $words[$wordIndex].Index)) {
                $match = $letters[$letterIndex++]
                $message = 'non-Latin letter "{0}"' -f $match.Value
            } else {
                $match = $words[$wordIndex++]
                $group = $match.Groups | Where-Object { $_.Success -and $_.Name -ne '0' }
                $american = $suggestions[[int]$group.Name.Substring(4)]
                $word = $line.Substring($match.Index, $match.Length)
                $message = 'British spelling "{0}": use {1}' -f $word, $american
            }
            '{0}:{1}:{2} {3}' -f $Path, ($i + 1), ($match.Index + 1), $message
        }
    }
}

function Find-FileTextIssues([string[]]$Paths) {
    foreach ($inputPath in $Paths) {
        $path = Get-RepoTextPath $inputPath
        if ($skipPaths.IsMatch($path)) { continue }
        $type = [IO.Path]::GetExtension($path)
        if (-not $type) { $type = [IO.Path]::GetFileName($path) }
        if ($type.ToLowerInvariant() -cin $binaryTypes) { continue }

        $absolute = [IO.Path]::GetFullPath($path, $textCheckRoot)
        $attributes = [IO.File]::GetAttributes($absolute)
        if ($attributes -band [IO.FileAttributes]::Directory) { continue }
        if (($attributes -band [IO.FileAttributes]::ReparsePoint) -and [IO.FileInfo]::new($absolute).LinkTarget) {
            continue
        }

        $bytes = [IO.File]::ReadAllBytes($absolute)
        $content = $null
        if ([Array]::IndexOf($bytes, [byte]0) -lt 0) {
            try { $content = $utf8.GetString($bytes) }
            catch [Text.DecoderFallbackException] { }
        }
        if ($null -eq $content) {
            if ($type.ToLowerInvariant() -cin $textTypes) { "${path}:1:1 not UTF-8 text" }
            continue
        }
        if ($content.StartsWith([char]0xfeff)) { $content = $content.Substring(1) }
        Find-TextIssues $path $content
    }
}

function Get-TrackedTextPaths {
    # Read Git's NUL-delimited bytes without normalizing newlines in filenames.
    $start = [Diagnostics.ProcessStartInfo]::new('git', 'ls-files -z')
    $start.WorkingDirectory = $textCheckRoot
    $start.RedirectStandardOutput = $true
    $start.UseShellExecute = $false
    $process = [Diagnostics.Process]::Start($start)
    $bytes = [IO.MemoryStream]::new()
    try {
        $process.StandardOutput.BaseStream.CopyTo($bytes)
        $process.WaitForExit()
        if ($process.ExitCode) { throw "git ls-files failed (exit $($process.ExitCode))" }
        foreach ($path in [Text.Encoding]::UTF8.GetString($bytes.ToArray()).Split([char]0, [StringSplitOptions]::RemoveEmptyEntries)) {
            [IO.Path]::Combine($textCheckRoot, $path)
        }
    } finally {
        $bytes.Dispose()
        $process.Dispose()
    }
}

if ($MyInvocation.InvocationName -ne '.') {
    $paths = if ($args.Count) { $args } else { Get-TrackedTextPaths }
    $findings = @(Find-FileTextIssues $paths)
    $findings
    if ($findings.Count) {
        [Console]::Error.WriteLine(("{0}{1} finding(s). A line that must stay as it is carries ""{2}""." -f
            [Environment]::NewLine, $findings.Count, $marker))
        exit 1
    }
    exit 0
}
