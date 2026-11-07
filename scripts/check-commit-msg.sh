#!/bin/sh
# The one rule for commit messages: authorship is git's author field and
# nothing else, so a message carries no AI attribution. Refused:
#   - a Co-Authored-By: trailer (any case, blanks before the colon allowed),
#   - the robot emoji (U+1F916),
#   - a "Generated with" attribution line: "Generated with " opening a line
#     (after blanks, #) and followed by a capitalized word, a [bracketed link]
#     or a URL. Prose like "generated with drizzle-kit" passes.
#
#   check-commit-msg.sh FILE            one message file (the commit-msg hook)
#   check-commit-msg.sh --range A..B    every commit in a range (CI)
#
# Plain sh and git, so it runs the same from Git for Windows' sh and on Linux.

LC_ALL=C
export LC_ALL

robot=$(printf '\360\237\244\226')

# Prints "line N (why): text" for the first offending line of the message on
# stdin and fails on one. A verbose commit's scissors line ends the message.
find_offence() {
    n=0
    while IFS= read -r line || [ -n "$line" ]; do
        n=$((n + 1))
        case $line in
            '# ------------------------ >8 ------------------------') break ;;
        esac
        why=
        case $line in
            *"$robot"*) why='robot emoji' ;;
        esac
        if [ -z "$why" ]; then
            if printf '%s\n' "$line" | grep -qiE '^[[:space:]]*co-authored-by[[:space:]]*:'; then
                why='Co-Authored-By trailer'
            elif printf '%s\n' "$line" | grep -qE '^[[:space:]#]*Generated with ([A-Z[]|https?://)'; then
                why='"Generated with" attribution'
            fi
        fi
        if [ -n "$why" ]; then
            printf '  line %s (%s): %s\n' "$n" "$why" "$line"
            return 1
        fi
    done
}

refuse() { # $1 = what was checked, $2 = the offense, $3 = the fix
    {
        echo "commit message refused ($1): no AI attribution in commits."
        echo "$2"
        echo
        echo "Authorship is git's author field only: drop the Co-Authored-By trailer, the"
        echo "robot emoji and any \"Generated with ...\" line. $3"
    } >&2
}

case $1 in
    --range)
        bad=0
        for c in $(git rev-list "$2"); do
            if ! out=$(git show -s --format=%B "$c" | find_offence); then
                refuse "commit $(git rev-parse --short "$c")" "$out" 'Rewrite the commit message before merging.'
                bad=1
            fi
        done
        exit $bad
        ;;
    '')
        echo 'usage: check-commit-msg.sh FILE | --range A..B' >&2
        exit 2
        ;;
    *)
        if ! out=$(find_offence <"$1"); then
            refuse 'commit-msg hook' "$out" 'Edit it with: git commit --amend'
            exit 1
        fi
        ;;
esac
