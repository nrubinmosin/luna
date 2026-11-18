#!/bin/sh
# Tests for scripts/macos-app.sh, off macOS: a dry run into a throwaway
# folder, the bundle's layout and Info.plist checked, and what it must
# refuse. Run: sh scripts/macos-app.test.sh

here=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
fail=0

check() { # $1 = what, then the test command
    what=$1
    shift
    if ! "$@"; then
        echo "FAIL: $what" >&2
        fail=1
    fi
}

printf 'an exe' >"$tmp/luna"
export LUNA_APP_DRY_RUN=1

if ! sh "$here/macos-app.sh" "$tmp/luna" 0.20.0 "$tmp/out" >/dev/null 2>"$tmp/err"; then
    echo 'FAIL: a dry run did not succeed' >&2
    cat "$tmp/err" >&2
    exit 1
fi
app="$tmp/out/Luna.app/Contents"
check 'the exe is in MacOS/ as luna' cmp -s "$tmp/luna" "$app/MacOS/luna"
check 'the exe is executable' test -x "$app/MacOS/luna"
check 'PkgInfo says APPL' grep -qx 'APPL????' "$app/PkgInfo"
plist="$app/Info.plist"
# Each key followed by its value, as the plist pairs them.
pair() { # $1 = key, $2 = value line
    grep -A1 "<key>$1</key>" "$plist" | tail -n 1 | grep -qF "$2"
}
check 'the identifier' pair CFBundleIdentifier '<string>rm.luna</string>'
check 'the executable' pair CFBundleExecutable '<string>luna</string>'
check 'the name' pair CFBundleName '<string>Luna</string>'
check 'the version' pair CFBundleShortVersionString '<string>0.20.0</string>'
check 'the build version' pair CFBundleVersion '<string>0.20.0</string>'
check 'the icon' pair CFBundleIconFile '<string>icon</string>'
check 'Retina' pair NSHighResolutionCapable '<true/>'
check 'the plist is closed' test "$(tail -n 1 "$plist")" = '</plist>'
check 'the dry run says what it skipped' grep -q 'dry run, no iconutil' "$tmp/err"

# Run again over the first: replaced, nothing of the old left.
touch "$app/stale"
check 'a second run succeeds' sh "$here/macos-app.sh" "$tmp/luna" 0.20.1 "$tmp/out" >/dev/null 2>&1
check 'a second run replaces the bundle' test ! -e "$app/stale"
check 'a second run writes its version' pair CFBundleVersion '<string>0.20.1</string>'

refused() { # $@ = the script's arguments, which it must refuse
    ! sh "$here/macos-app.sh" "$@" "$tmp/refused" 2>/dev/null
}
for bad in '' 1.2 1.2.3.4 v1.2.3 1.2.x 1..2 '1.2.3 ' 1.2.3-rc.1; do
    check "the version '$bad' is refused" refused "$tmp/luna" "$bad"
done
check 'a missing exe is refused' refused "$tmp/missing" 0.20.0
check 'nothing was made for a refusal' test ! -e "$tmp/refused/Luna.app"
if ! command -v plutil >/dev/null 2>&1; then
    check 'off macOS, only a dry run goes through' env LUNA_APP_DRY_RUN=0 sh -c '! sh "$0" "$1" 0.20.0 "$2" 2>/dev/null' \
        "$here/macos-app.sh" "$tmp/luna" "$tmp/real"
fi

if [ $fail -ne 0 ]; then
    exit 1
fi
echo 'macos-app: all cases pass'
