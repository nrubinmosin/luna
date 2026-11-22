#!/bin/sh
# Assembles Luna.app, unsigned, from a macOS build of the exe: what macOS
# needs to run Luna as an app, with its icon, its bundle identifier (which
# notifications go out under, src-tauri/src/desktop/notify.rs) and its
# version. For CI's macOS runner; macOS is built and shipped experimental
# (ADR-0030).
#
#   sh scripts/macos-app.sh <exe> <version> <out-dir>
#
# Writes <out-dir>/Luna.app, replacing one there. The icon is made with
# sips and iconutil, and the bundle sealed with an ad-hoc signature
# (codesign -s -, no identity): macOS then calls a downloaded copy one
# from an unidentified developer, which a person may open, rather than
# damaged. Off macOS (LUNA_APP_DRY_RUN=1, scripts/macos-app.test.sh) those
# tools are not there and their steps are skipped, said on stderr.
set -eu

fail() {
    echo "macos-app: $*" >&2
    exit 1
}

[ $# -eq 3 ] || fail 'usage: macos-app.sh <exe> <version> <out-dir>'
exe=$1
version=$2
out=$3
here=$(cd "$(dirname "$0")" && pwd)
icon_png="$here/../src-tauri/icons/icon.png"
dry=${LUNA_APP_DRY_RUN:-0}

[ -f "$exe" ] || fail "no exe at $exe"
case $version in
    *[!0-9.]* | .* | *. | *..*) fail "not an x.y.z version: $version" ;;
esac
[ "$(echo "$version" | tr -cd . | wc -c)" -eq 2 ] || fail "not an x.y.z version: $version"
[ -f "$icon_png" ] || fail "no icon at $icon_png"

# Present only on macOS; a run anywhere else has to say it is a dry one.
tool() {
    if command -v "$1" >/dev/null 2>&1; then
        return 0
    fi
    [ "$dry" = 1 ] || fail "$1 is not on PATH (macOS only; LUNA_APP_DRY_RUN=1 skips it)"
    echo "macos-app: dry run, no $1: $2 skipped" >&2
    return 1
}

app="$out/Luna.app"
mkdir -p "$out"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$exe" "$app/Contents/MacOS/rm-luna"
chmod 755 "$app/Contents/MacOS/rm-luna"
printf 'APPL????' >"$app/Contents/PkgInfo"

# Keep Luna's existing bundle identity. GPUI needs Retina rendering and
# macOS 10.15.7 or later.
cat >"$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>en</string>
	<key>CFBundleDisplayName</key>
	<string>Luna</string>
	<key>CFBundleExecutable</key>
	<string>rm-luna</string>
	<key>CFBundleIconFile</key>
	<string>icon</string>
	<key>CFBundleIdentifier</key>
	<string>rm.luna</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>Luna</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>$version</string>
	<key>CFBundleVersion</key>
	<string>$version</string>
	<key>LSApplicationCategoryType</key>
	<string>public.app-category.developer-tools</string>
	<key>LSMinimumSystemVersion</key>
	<string>10.15.7</string>
	<key>NSHighResolutionCapable</key>
	<true/>
</dict>
</plist>
EOF

if tool plutil 'the Info.plist check'; then
    plutil -lint "$app/Contents/Info.plist" >/dev/null
fi

# The icon set iconutil wants, from the 512 px PNG: each size and its @2x,
# but for 512's, which would take a 1024 px source.
if tool iconutil 'the icon' && tool sips 'the icon'; then
    set_dir=$(mktemp -d)/icon.iconset
    mkdir -p "$set_dir"
    for size in 16 32 128 256 512; do
        sips -z $size $size "$icon_png" --out "$set_dir/icon_${size}x${size}.png" >/dev/null
        double=$((size * 2))
        if [ $double -le 512 ]; then
            sips -z $double $double "$icon_png" --out "$set_dir/icon_${size}x${size}@2x.png" >/dev/null
        fi
    done
    iconutil -c icns "$set_dir" -o "$app/Contents/Resources/icon.icns"
    rm -rf "$(dirname "$set_dir")"
fi

if tool codesign 'the ad-hoc seal'; then
    codesign --force --sign - "$app"
fi

echo "macos-app: $app ($version)"
