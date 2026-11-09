#Requires -Version 7.4
# Checks that the UI fits small screens, in Docker, on the demo data of
# src/dev/demo/ (as screenshots.ps1 shoots it): every scene, and the popups a
# click opens in them, at 1280x720 and 1024x600, the first at 125% and 150%,
# and 1024x600 at 150%. Fails on any control of a dialog, popover or menu that
# is neither on screen nor in a scroller that is, on a popup cut off where it
# stands, and on one left behind by a scroll (scripts/fit-check.mjs).
#
#   ./scripts/fit-check.ps1
#   ./scripts/fit-check.ps1 -Scenes newchat,newchat-picker -Sizes 1280x720@150
#
# A shot of each scene and size and report.json go to -Out, relative to the
# repo root (test-build/fit by default, which git ignores). Not part of the
# gate: it takes some ten minutes.
param(
    [string]$Out = 'test-build/fit',
    [string[]]$Scenes = @(),
    [string[]]$Sizes = @()
)
. "$PSScriptRoot/lib.ps1"

$image = Build-Image luna-shots screenshots.Dockerfile

Initialize-HostDirs 'node_modules', 'test-build'
New-Item -ItemType Directory -Force (Join-Path $RepoRoot $Out) | Out-Null

# The UI's own faces, as screenshots.ps1 mounts them: text in a substitute
# face wraps elsewhere, and where text wraps is what decides what fits.
$faces = 'SegUIVar.ttf', 'segoeui.ttf', 'segoeuib.ttf', 'seguisb.ttf', 'tahoma.ttf', 'tahomabd.ttf',
         'trebuc.ttf', 'trebucbd.ttf', 'trebucit.ttf', 'trebucbi.ttf'
$fontArgs = @()
if ($IsWindows) {
    $fontDir = [Environment]::GetFolderPath('Fonts')
    foreach ($f in $faces) {
        $src = Join-Path $fontDir $f
        if (Test-Path -LiteralPath $src) { $fontArgs += @('-v', "${src}:/usr/share/fonts/truetype/windows/${f}:ro") }
    }
}
if (-not $fontArgs) { Write-Warning 'no Windows fonts found — text will wrap as in other faces' }

# Single-quoted: the container's shell owns every $ in here.
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
node scripts/fit-check.mjs /app/$FIT_OUT ${FIT_SCENES:--} ${FIT_SIZES:--}
'@

# The node_modules volume is the Windows build's too (scripts/lib.ps1 Use-BuildLock).
Use-BuildLock {
    Invoke-Checked 'fit check' {
        docker run --rm `
          -e COREPACK_ENABLE_DOWNLOAD_PROMPT=0 `
          -e FIT_OUT=$Out `
          -e FIT_SCENES=$($Scenes -join ',') `
          -e FIT_SIZES=$($Sizes -join ',') `
          @fontArgs `
          -v "${RepoRoot}:/app" `
          -v luna-node-modules:/app/node_modules `
          -w /app `
          $image `
          bash -c $cmd
    }
}
