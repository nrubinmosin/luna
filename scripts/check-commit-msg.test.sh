#!/bin/sh
# Tests for scripts/check-commit-msg.sh: messages it must refuse and accept.
# Run: sh scripts/check-commit-msg.test.sh

here=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT
robot=$(printf '\360\237\244\226')
fail=0

check() { # $1 = refuse|accept, $2 = message
    printf '%s\n' "$2" >"$tmp"
    if sh "$here/check-commit-msg.sh" "$tmp" >/dev/null 2>&1; then got=accept; else got=refuse; fi
    if [ "$got" != "$1" ]; then
        echo "FAIL: expected $1, got $got: $2" >&2
        fail=1
    fi
}

check refuse 'Fix it

Co-Authored-By: Claude <noreply@anthropic.com>'
check refuse 'Fix it

co-authored-by: Someone <a@b.c>'
check refuse 'Fix it

CO-AUTHORED-BY : Someone <a@b.c>'
check refuse "Fix it $robot"
check refuse "$robot Generated with Claude Code"
check refuse 'Generated with Claude Code'
check refuse '  Generated with [Claude Code](https://claude.com/claude-code)'
check refuse 'Generated with https://example.com'
check refuse '# Generated with Tool'

check accept 'Fix it'
check accept 'Rebuild the schema (generated with drizzle-kit)'
check accept 'generated with drizzle-kit'
check accept 'Generated with care'
check accept 'Explain why a Co-Authored-By trailer is refused'
check accept '# Please enter the commit message
# ------------------------ >8 ------------------------
Co-Authored-By: x'

[ "$fail" = 0 ] && echo 'check-commit-msg: all cases passed'
exit $fail
