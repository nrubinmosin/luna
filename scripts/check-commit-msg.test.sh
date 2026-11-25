#!/bin/sh
# Tests for scripts/check-commit-msg.sh: messages it must refuse and accept, in
# the hook, stored message and range modes (commits in a throwaway repository
# made here). Run: sh scripts/check-commit-msg.test.sh
# Needs git on PATH: the range mode is git.

here=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp)
repo=$(mktemp -d)
trap 'rm -rf "$tmp" "$repo"' EXIT
robot=$(printf '\360\237\244\226')
fail=0

if ! command -v git >/dev/null 2>&1; then
    echo 'check-commit-msg: git is needed for the range cases' >&2
    exit 2
fi

# A repository of its own, with no hooks or signing from the user's config
# reaching into it, and a base commit the range cases start from.
g() { git -C "$repo" "$@"; }
g init -q
g config user.name test
g config user.email test@example.com
g config commit.gpgsign false
g commit -q --allow-empty --no-verify -m 'Base'
base=$(g rev-parse HEAD)

report() { # $1 = mode, $2 = expected, $3 = got, $4 = message
    echo "FAIL ($1): expected $2, got $3: $4" >&2
    fail=1
}

check() { # $1 = refuse|accept, $2 = message, remaining args precede the file
    expected=$1
    message=$2
    shift 2
    printf '%s\n' "$message" >"$tmp"
    if sh "$here/check-commit-msg.sh" "$@" "$tmp" >/dev/null 2>&1; then got=accept; else got=refuse; fi
    [ "$got" = "$expected" ] || report "file $*" "$expected" "$got" "$message"
}

# One commit with the message on top of the base, checked as a range, then
# taken off again.
check_range() { # $1 = refuse|accept, $2 = message, remaining args precede --range
    expected=$1
    message=$2
    shift 2
    printf '%s\n' "$message" >"$tmp"
    g commit -q --allow-empty --no-verify -F "$tmp"
    if (cd "$repo" && sh "$here/check-commit-msg.sh" "$@" --range "$base..HEAD") >/dev/null 2>&1; then got=accept; else got=refuse; fi
    [ "$got" = "$expected" ] || report "range $*" "$expected" "$got" "$message"
    g reset -q --hard "$base"
}

# Prints the refusal for a message, for checking what it names.
explain() { # $1 = message
    printf '%s\n' "$1" >"$tmp"
    sh "$here/check-commit-msg.sh" "$tmp" 2>&1
}

names() { # $1 = message, $2 = text the refusal must carry
    if ! explain "$1" | grep -qF -- "$2"; then
        echo "FAIL (hook): the refusal of '$1' does not say: $2" >&2
        fail=1
    fi
}

lacks() { # $1 = message, $2 = text the refusal must not carry
    if explain "$1" | grep -qF -- "$2"; then
        echo "FAIL (hook): the refusal of '$1' says: $2" >&2
        fail=1
    fi
}

# --- attribution (every commit, whatever its form) ---

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
check accept 'Generated with care'
check accept 'Explain why a Co-Authored-By trailer is refused'
check accept '# Please enter the commit message
# ------------------------ >8 ------------------------
Co-Authored-By: x'

# --- the subject ---

long='The subject of this commit runs on and on past the seventy-two characters'
check accept 'An elevated run takes back an open file it made, owned by Administrators'
check refuse "$long"
names "$long" 'longer than 72'
check refuse 'Fix the gate.'
names 'Fix the gate.' 'ends in a period'
check accept 'Fix the gate: step 2.0'
check refuse 'generated with drizzle-kit'
check refuse 'fix the gate'
names 'fix the gate' 'lowercase'
# A refused commit was never made: the way on is a new commit from the
# message, not an amend, which would fold the staged change into the one before.
names 'fix the gate' "git commit -e -F $tmp"
lacks 'fix the gate' 'Edit it with: git commit --amend'
check accept '2026 releases are attested'
check accept 'agents/wait.rs: luna_wait on one session or several'
check accept 'check-text passes over symbolic links'
check accept '.githooks/commit-msg: one rule, in scripts/'
check accept 'luna_wait returns early when the session is gone'
# A path prefix does not stretch the limit.
check refuse 'agents/notifier/lines.rs: what a notice says, line by line, and why the limit still holds'
check refuse 'docs: the spawn sequence is schematic'
names 'docs: the spawn sequence is schematic' 'type prefix'
check refuse 'fix(scope)!: the gate'
check refuse 'Docs: the spawn sequence is schematic'
check refuse 'feat: add a thing'
check accept 'Fixes for the gate'
check accept 'Documentation of the spawn sequence'

# --- the body ---

check refuse 'Fix the gate
The body starts without a blank line'
names 'Fix the gate
The body starts without a blank line' 'line 2'
check accept 'Fix the gate

The body starts after a blank line.'
check refuse 'Fix the gate

This body line runs on past the seventy-two character limit of a wrapped body'
names 'Fix the gate

This body line runs on past the seventy-two character limit of a wrapped body' 'longer than 72'
check accept 'Fix the gate

See https://github.com/nrubinmosin/luna/actions/runs/1234567890/job/12345678901234567890'
check accept 'Fix the gate

    an indented line, a quoted command or a table row, may run on past seventy-two
'
check accept 'Fix the gate

secret::tests::a_file_planted_where_one_is_written_in_place_is_moved_aside_again'
# The hook sees git's comment lines; they are no part of the message.
check accept '# Please enter the commit message for your changes. Lines starting
Fix the gate
# with # will be ignored, and an empty message aborts the commit, and so on

The body.'
check accept '

Fix the gate'

# --- exemptions ---

check accept 'Merge branch '"'"'refactor/phase-5-docs'"'"' into refactor/campaign, with a long subject line.'
check accept 'Merge pull request #5 from nrubinmosin/release/v0.10.0'
check accept 'Revert "fix the gate"

This reverts commit 0123456789abcdef0123456789abcdef01234567.'
check accept 'Luna 0.10.0'
check refuse 'Luna 0.10.0.'
check refuse 'fixup! fix the gate'
check refuse 'squash! fix the gate.'
# What git commit --fixup=amend: and --fixup=reword: write.
check refuse 'amend! fix the gate

Fix the gate'
check refuse 'fixup! fix the gate

Co-Authored-By: Someone <a@b.c>'
check refuse 'amend! Fix the gate

Co-Authored-By: Someone <a@b.c>'

# --- the rules follow the branch ---

attribution='Fix the gate

Co-Authored-By: Someone <a@b.c>'
check accept "lowercase $long" --branch slice/gate
check accept 'fixup! Fix the gate' --branch slice/gate
check accept 'squash! Fix the gate' --branch feature/gate
check accept 'amend! Fix the gate' --branch feature/gate
check refuse "$attribution" --branch slice/gate
for branch in master iteration/gate release/v0.10.0 ''; do
    check refuse "lowercase $long" --branch "$branch"
    check refuse 'fixup! Fix the gate' --branch "$branch"
    check refuse "$attribution" --branch "$branch"
done
check_range accept "lowercase $long" --branch slice/gate
check_range accept 'fixup! Fix the gate' --branch slice/gate
check_range refuse "$attribution" --branch slice/gate
check_range refuse "lowercase $long" --branch iteration/gate
check_range refuse 'fixup! Fix the gate' --branch release/v0.10.0
check_range refuse 'fixup! Fix the gate' --branch ''

# --- standalone stored messages (the PR title and body) ---

check accept 'Fix the gate (#12)

The body starts after a blank line.' --message
check accept 'Fix the gate (#12)' --message
check refuse "$long (#12)" --message
check refuse 'fix the gate (#12)' --message
check refuse 'fixup! Fix the gate (#12)' --message
check accept 'Merge the gate changes (#12)' --message
check refuse "Merge $long (#12)" --message
check refuse 'Fix the gate (#12)
# A heading that would be a comment in the hook

The body.' --message
check refuse 'Fix the gate (#12)

# ------------------------ >8 ------------------------
Co-Authored-By: x' --message

# --- the range mode ---

check_range accept 'An elevated run takes back an open file it made, owned by Administrators

The body, wrapped at seventy-two.'
check_range refuse "$long"
check_range refuse 'fix the gate'
check_range refuse 'Fix the gate

Co-Authored-By: Someone <a@b.c>'
check_range refuse 'fixup! Fix the gate'
check_range refuse 'squash! Fix the gate'
check_range refuse 'amend! Fix the gate

Fix the gate'
check_range accept 'Luna 0.10.0'

# The range as it is stored: no comment lines, no scissors, and a bad range is
# an error, not an empty one.
check_range refuse '# Please enter the commit message
# ------------------------ >8 ------------------------
Co-Authored-By: x'
check_range refuse 'Fix the gate

# ------------------------ >8 ------------------------
Co-Authored-By: x'
(cd "$repo" && sh "$here/check-commit-msg.sh" --range "no-such-branch..HEAD") >/dev/null 2>&1
[ $? = 2 ] || { echo 'FAIL (range): a bad range should exit 2' >&2; fail=1; }

# --- CRLF: the CR is no part of the line, in either mode ---

cr=$(printf '\r')
seventytwo='An elevated run takes back an open file it made, owned by Administrators'
check accept "$seventytwo$cr"
check refuse "Fix the gate.$cr"
check accept "Luna 0.10.0$cr"
check accept "Fix the gate$cr
$cr
The body, each line ending in a CR.$cr"
# git's own cleanup strips a CR; verbatim keeps it, as a message in the store.
range_verbatim() { # $1 = refuse|accept, $2 = message
    printf '%s\n' "$2" >"$tmp"
    g commit -q --allow-empty --no-verify --cleanup=verbatim -F "$tmp"
    if (cd "$repo" && sh "$here/check-commit-msg.sh" --range "$base..HEAD") >/dev/null 2>&1; then got=accept; else got=refuse; fi
    [ "$got" = "$1" ] || report range "$1" "$got" "$2"
    g reset -q --hard "$base"
}
range_verbatim accept "$seventytwo$cr"
range_verbatim refuse "Fix the gate.$cr"
range_verbatim accept "Luna 0.10.0$cr"

# --- the hook follows core.commentChar ---

check_in_repo() { # $1 = refuse|accept, $2 = message; the hook mode, in the repository
    printf '%s\n' "$2" >"$tmp"
    if (cd "$repo" && sh "$here/check-commit-msg.sh" "$tmp") >/dev/null 2>&1; then got=accept; else got=refuse; fi
    [ "$got" = "$1" ] || report "hook, commentChar=$(g config core.commentChar || echo unset)" "$1" "$got" "$2"
}
check_in_repo accept 'Fix the gate
# a comment, with the default character

The body.'
g config core.commentChar ';'
check_in_repo accept 'Fix the gate
; Please enter the commit message for your changes. Lines starting

The body.'
check_in_repo refuse 'Fix the gate
# a heading is text now, where a blank line should be

The body.'
check_in_repo accept 'Fix the gate

The body.
; ------------------------ >8 ------------------------
Co-Authored-By: x'
check_in_repo refuse 'Fix the gate

The body.
# ------------------------ >8 ------------------------
Co-Authored-By: x'
# commentString and commentChar are one setting: the last configured wins. (A
# fresh key each time: git replaces a set one in place, keeping its position.)
g config --unset core.commentChar
g config core.commentString '::'
g config core.commentChar ';'
check_in_repo accept 'Fix the gate
; a comment

The body.'
check_in_repo refuse 'Fix the gate
:: text, not a comment

The body.'
g config --unset core.commentString
g config --unset core.commentChar
g config core.commentChar ';'
g config core.commentString '::'
check_in_repo accept 'Fix the gate
:: a comment

The body.'
check_in_repo refuse 'Fix the gate
; text, not a comment

The body.'
g config --unset core.commentString
g config --unset core.commentChar
# With "auto", git's pick is unknown, so nothing is a comment and no scissors
# line cuts: a message cannot choose a character and hide a trailer behind it.
g config core.commentChar auto
check_in_repo refuse 'Fix the gate

The body.
; ------------------------ >8 ------------------------
Co-Authored-By: x'
check_in_repo refuse 'Fix the gate

The body.
# ------------------------ >8 ------------------------
Co-Authored-By: x'
check_in_repo accept 'Fix the gate

# A heading
;
; Please enter the commit message for your changes. Lines starting
; with ";" will be ignored, and an empty message aborts the commit.'
check_in_repo refuse 'Fix the gate
# a comment elsewhere, text here

The body.'
printf 'Fix the gate\n' >"$tmp"
if ! (cd "$repo" && sh "$here/check-commit-msg.sh" "$tmp") 2>&1 | grep -q 'core.commentChar is auto'; then
    echo 'FAIL (hook, commentChar=auto): no warning that comment lines are checked as text' >&2
    fail=1
fi
g config --unset core.commentChar

# --- merges and what a range leaves out ---

# A merge commit's form is git's own, but its attribution is checked.
g switch -q -c side
g commit -q --allow-empty --no-verify -m 'Side'
g switch -q -
g merge -q --no-ff --no-verify -m 'merged the side branch.' side
if (cd "$repo" && sh "$here/check-commit-msg.sh" --range "$base..HEAD") >/dev/null 2>&1; then got=accept; else got=refuse; fi
[ "$got" = accept ] || report range accept "$got" 'a merge commit'
g reset -q --hard "$base"
g merge -q --no-ff --no-verify -m 'Merge branch side

Co-Authored-By: Someone <a@b.c>' side
if (cd "$repo" && sh "$here/check-commit-msg.sh" --range "$base..HEAD") >/dev/null 2>&1; then got=accept; else got=refuse; fi
[ "$got" = refuse ] || report range refuse "$got" 'a merge commit with attribution'
g reset -q --hard "$base"
g branch -q -D side

# What another branch already holds (master, merged into a topic branch) is
# left out of the range with ^branch, as CI does off master.
g switch -q -c other
g commit -q --allow-empty --no-verify -m 'a subject already on the other branch, lowercase and all.'
g switch -q -
g commit -q --allow-empty --no-verify -m 'Fix the gate'
g merge -q --no-ff --no-verify -m 'Merge branch other' other
if (cd "$repo" && sh "$here/check-commit-msg.sh" --range "$base..HEAD") >/dev/null 2>&1; then got=accept; else got=refuse; fi
[ "$got" = refuse ] || report range refuse "$got" 'the other branch merged in, nothing left out'
if (cd "$repo" && sh "$here/check-commit-msg.sh" --range "$base..HEAD" ^other) >/dev/null 2>&1; then got=accept; else got=refuse; fi
[ "$got" = accept ] || report range accept "$got" 'the other branch merged in and left out'
g reset -q --hard "$base"
g branch -q -D other

# --- a detached checkout uses the branch named by the active rebase ---

mkdir "$repo/scripts"
cp "$here/check-commit-msg.sh" "$repo/scripts/"
g switch -q --detach "$base"
check_detached_hook() { # $1 = refuse|accept, $2 = case description
    printf 'fixup! Clean up the gate\n' >"$tmp"
    if (cd "$repo" && sh "$here/../.githooks/commit-msg" "$tmp") >/dev/null 2>&1; then got=accept; else got=refuse; fi
    [ "$got" = "$1" ] || report 'commit-msg hook' "$1" "$got" "$2"
}
check_detached_hook refuse 'a detached HEAD outside a rebase'
for backend in rebase-merge rebase-apply; do
    state="$repo/.git/$backend"
    mkdir "$state"
    printf 'refs/heads/slice/gate\n' >"$state/head-name"
    check_detached_hook accept "a slice branch in $backend"
    printf 'refs/heads/iteration/gate\n' >"$state/head-name"
    check_detached_hook refuse "a kept-history branch in $backend"
    rm "$state/head-name"
    rmdir "$state"
done

[ "$fail" = 0 ] && echo 'check-commit-msg: all cases passed'
exit $fail
