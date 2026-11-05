# Retakes the README screenshots, in Docker, without going near a running Luna.
# Shooting the real app means shooting whatever is open in it; this runs the
# frontend alone (Tauri commands are no-ops) on the invented data in
# src/dev/demo.ts and photographs that.
#
#   ./screenshots.ps1
#   ./screenshots.ps1 -Size 1600,1000
#   ./screenshots.ps1 -Out test-build/shots -Scenes checks,checks-dark
#
# -Scenes shoots those of src/dev/demo.ts, each to a file of its name, instead of
# the three the README shows.
param(
    [string]$Out = 'docs/screenshots',
    [string]$Size = '1760,1040',
    [string[]]$Scenes = @()
)
$ErrorActionPreference = 'Stop'

# `-Size 1760,1040` without quotes is two arguments to PowerShell, which joins
# them with a space on the way into a [string] — and Chromium then quietly
# shoots at some default size instead.
$Size = ($Size -replace '\s+', ',')

docker build -t luna-shots -f docker/screenshots.Dockerfile docker

New-Item -ItemType Directory -Force $Out | Out-Null

# The UI is Segoe UI with Tahoma behind it, and title bars and the New chat
# button are Trebuchet MS, as on XP. Without the real faces every shot comes
# out in a substitute, i.e. not in the app's own type.
$faces = 'SegUIVar.ttf', 'segoeui.ttf', 'segoeuib.ttf', 'seguisb.ttf', 'tahoma.ttf', 'tahomabd.ttf',
         'trebuc.ttf', 'trebucbd.ttf', 'trebucit.ttf', 'trebucbi.ttf'
$fontArgs = @()
foreach ($f in $faces) {
    $src = Join-Path 'C:\Windows\Fonts' $f
    if (Test-Path $src) { $fontArgs += @('-v', "${src}:/usr/share/fonts/truetype/windows/${f}:ro") }
}
if (-not $fontArgs) { Write-Warning 'no Windows fonts found — the shots will fall back to other faces' }

# Single-quoted: the container's shell owns every $ in here. No double quotes:
# Windows PowerShell breaks up a native argument with one in it. With IFS empty
# nothing past that line is split, so a -Out with a space in it needs none.
$cmd = @'
set -e
fc-cache -f >/dev/null
pnpm install --frozen-lockfile
pnpm exec vite --host 0.0.0.0 --port 1420 >/tmp/vite.log 2>&1 &
for i in $(seq 1 90); do
  curl -sfo /dev/null http://localhost:1420/ && break
  sleep 1
done
IFS=
shoot() {
  chromium --headless=new --no-sandbox --disable-gpu --hide-scrollbars \
    --window-size=$SHOT_SIZE --virtual-time-budget=8000 \
    --screenshot=$SHOT_OUT/$1.png http://localhost:1420/\?demo=$2
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
'@ -replace "`r`n", "`n"

$sceneList = $Scenes -join ','
docker run --rm `
  -e COREPACK_ENABLE_DOWNLOAD_PROMPT=0 `
  -e SHOT_SIZE=$Size `
  -e SHOT_OUT="/app/$Out" `
  -e SHOT_SCENES=$sceneList `
  @fontArgs `
  -v "${PWD}:/app" `
  -v luna-node-modules:/app/node_modules `
  -w /app `
  luna-shots `
  bash -c $cmd

Get-ChildItem $Out -Filter *.png | ForEach-Object { "{0} ({1:N0} KB)" -f $_.Name, ($_.Length / 1KB) }
