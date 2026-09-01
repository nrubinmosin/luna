# Retakes the README screenshots, in Docker, without going near a running Luna.
# Shooting the real app means shooting whatever is open in it; this runs the
# frontend alone (Tauri commands are no-ops) on the invented data in
# src/dev/demo.ts and photographs that.
#
#   ./screenshots.ps1
#   ./screenshots.ps1 -Size 1600,1000
param(
    [string]$Out = 'docs/screenshots',
    [string]$Size = '1760,1040'
)
$ErrorActionPreference = 'Stop'

# `-Size 1760,1040` without quotes is two arguments to PowerShell, which joins
# them with a space on the way into a [string] — and Chromium then quietly
# shoots at some default size instead.
$Size = ($Size -replace '\s+', ',')

docker build -t luna-shots -f docker/screenshots.Dockerfile docker

New-Item -ItemType Directory -Force $Out | Out-Null

# The UI is Tahoma, the way XP was. Without the real face every shot comes out
# in a substitute, i.e. not in the app's own type.
$tahoma = 'C:\Windows\Fonts\tahoma.ttf'
$fontArgs = @()
if (Test-Path $tahoma) {
    $fontArgs = @('-v', "${tahoma}:/usr/share/fonts/truetype/tahoma/tahoma.ttf:ro")
} else {
    Write-Warning "no $tahoma — the shots will fall back to another sans face"
}

# Single-quoted: the container's shell owns every $ in here.
$cmd = @'
set -e
fc-cache -f >/dev/null
pnpm install --frozen-lockfile
pnpm exec vite --host 0.0.0.0 --port 1420 >/tmp/vite.log 2>&1 &
for i in $(seq 1 90); do
  curl -sfo /dev/null http://localhost:1420/ && break
  sleep 1
done
shoot() {
  chromium --headless=new --no-sandbox --disable-gpu --hide-scrollbars \
    --window-size="$SHOT_SIZE" --virtual-time-budget=8000 \
    --screenshot="$SHOT_OUT/$1.png" "http://localhost:1420/?demo=$2"
  echo "shot $1"
}
mkdir -p "$SHOT_OUT"
shoot panes main
shoot new-chat newchat
shoot dark dark
'@ -replace "`r`n", "`n"

docker run --rm `
  -e COREPACK_ENABLE_DOWNLOAD_PROMPT=0 `
  -e SHOT_SIZE=$Size `
  -e SHOT_OUT="/app/$Out" `
  @fontArgs `
  -v "${PWD}:/app" `
  -v luna-node-modules:/app/node_modules `
  -w /app `
  luna-shots `
  bash -c $cmd

Get-ChildItem $Out -Filter *.png | ForEach-Object { "{0} ({1:N0} KB)" -f $_.Name, ($_.Length / 1KB) }
