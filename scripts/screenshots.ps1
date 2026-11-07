#Requires -Version 7.4
# Retakes the README screenshots, in Docker, without going near a running Luna.
# Shooting the real app means shooting whatever is open in it; this runs the
# frontend alone (Tauri commands are no-ops) on the invented data in
# src/dev/demo/ and photographs that.
#
#   ./scripts/screenshots.ps1
#   ./scripts/screenshots.ps1 -Size 1600,1000
#   ./scripts/screenshots.ps1 -Out test-build/shots -Scenes checks,checks-dark
#
# -Out is relative to the repo root. -Scenes shoots those of src/dev/demo/scenes.ts,
# each to a file of its name, instead of the three the README shows.
param(
    [string]$Out = 'docs/screenshots',
    [string]$Size = '1760,1040',
    [string[]]$Scenes = @()
)
. "$PSScriptRoot/lib.ps1"

# `-Size 1760,1040` without quotes is two arguments to PowerShell, which joins
# them with a space on the way into a [string] — and Chromium then quietly
# shoots at some default size instead.
$Size = ($Size -replace '\s+', ',')

$image = Build-Image luna-shots screenshots.Dockerfile

Initialize-HostDirs 'node_modules'
$outDir = Join-Path $RepoRoot $Out
New-Item -ItemType Directory -Force $outDir | Out-Null

# The UI is Segoe UI with Tahoma behind it, and title bars and the New chat
# button are Trebuchet MS, as on XP. Without the real faces every shot comes
# out in a substitute, i.e. not in the app's own type.
$faces = 'SegUIVar.ttf', 'segoeui.ttf', 'segoeuib.ttf', 'seguisb.ttf', 'tahoma.ttf', 'tahomabd.ttf',
         'trebuc.ttf', 'trebucbd.ttf', 'trebucit.ttf', 'trebucbi.ttf'
$fontArgs = @()
# Only Windows has them, and only there is C: a drive Join-Path will take.
if ($IsWindows) {
    $fontDir = [Environment]::GetFolderPath('Fonts')
    foreach ($f in $faces) {
        $src = Join-Path $fontDir $f
        if (Test-Path -LiteralPath $src) { $fontArgs += @('-v', "${src}:/usr/share/fonts/truetype/windows/${f}:ro") }
    }
}
if (-not $fontArgs) { Write-Warning 'no Windows fonts found — the shots will fall back to other faces' }

# Single-quoted: the container's shell owns every $ in here. With IFS empty
# nothing past that line is split, so a -Out with a space in it needs none.
$cmd = ConvertTo-BashScript @'
set -e
fc-cache -f >/dev/null
pnpm install --frozen-lockfile
pnpm exec vite --host 0.0.0.0 --port 1420 >/tmp/vite.log 2>&1 &
vite=$!
up=
for i in $(seq 1 90); do
  if curl -sfo /dev/null http://localhost:1420/; then up=1; break; fi
  kill -0 $vite 2>/dev/null || break
  sleep 1
done
if test -z $up; then
  echo 'vite did not come up on :1420; its log:' >&2
  cat /tmp/vite.log >&2
  exit 1
fi
IFS=
# Chromium can exit 0 without having written the shot, so the file is the test.
shoot() {
  rm -f $SHOT_OUT/$1.png
  chromium --headless=new --no-sandbox --disable-gpu --hide-scrollbars \
    --window-size=$SHOT_SIZE --virtual-time-budget=8000 \
    --screenshot=$SHOT_OUT/$1.png http://localhost:1420/\?demo=$2
  test -s $SHOT_OUT/$1.png || { echo no shot $1 >&2; exit 1; }
  echo shot $1
}
mkdir -p $SHOT_OUT
if test ${#SHOT_SCENES} -gt 0; then
  IFS=,
  set -- $SHOT_SCENES
  IFS=
  for scene; do shoot $scene $scene; done
else
  shoot panes main
  shoot new-chat newchat
  shoot dark dark
fi
'@

$sceneList = $Scenes -join ','
# The node_modules volume is the Windows build's too (scripts/lib.ps1 Use-BuildLock).
Use-BuildLock {
    Invoke-Checked 'screenshots' {
        docker run --rm `
          -e COREPACK_ENABLE_DOWNLOAD_PROMPT=0 `
          -e SHOT_SIZE=$Size `
          -e SHOT_OUT="/app/$Out" `
          -e SHOT_SCENES=$sceneList `
          @fontArgs `
          -v "${RepoRoot}:/app" `
          -v luna-node-modules:/app/node_modules `
          -w /app `
          $image `
          bash -c $cmd
    }
}

Get-ChildItem $outDir -Filter *.png | ForEach-Object { "{0} ({1:N0} KB)" -f $_.Name, ($_.Length / 1KB) }
