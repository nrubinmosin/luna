#!/bin/sh
# The rules for commit messages (CONTRIBUTING.md, "Commits"), shared by the
# commit-msg hook, CI and the check of the PR title and body.
#
# Every commit: authorship is git's author field and nothing else, so a
# message carries no AI attribution. Refused:
#   - a Co-Authored-By: trailer (any case, blanks before the colon allowed),
#   - the robot emoji (U+1F916),
#   - a "Generated with" attribution line: "Generated with " opening a line
#     (after blanks, #) and followed by a capitalized word, a [bracketed link]
#     or a URL. Prose like "generated with drizzle-kit" passes.
#
# The form applies to master, iteration/* and release/*, where history is
# kept, and to the PR message. Other branches check only attribution.
# The form is a sentence saying what the change does:
#   - the subject is at most 72 characters and does not end in a period,
#   - it starts with a capital letter, a digit or a path-like name
#     ("agents/wait.rs:", "check-text", "luna_wait"); lowercase prose is
#     refused, and so is a Conventional Commits type prefix ("docs:",
#     "fix(scope)!:"): the type is what the sentence says,
#   - line 2 is blank, and the body is wrapped at 72, except a line with a
#     URL, an indented line and a line that is one unbreakable token.
# Exempt from the form: a merge ("Merge ..."), a revert ('Revert "..."') and
# the release bump ("Luna X.Y.Z").
# "fixup!", "squash!" and "amend!" commits are only allowed on branches whose
# commits are squashed.
#
#   check-commit-msg.sh [--branch NAME] FILE             the commit-msg hook
#   check-commit-msg.sh [--branch NAME] --range REVS...  every selected commit
#   check-commit-msg.sh --message FILE                   a stored message
# An omitted or empty branch name checks the full form.
#
# The hook sees the message as git hands it over, before the cleanup: comment
# lines (core.commentChar) and, after a scissors line, the diff of commit -v are
# no part of it and are skipped for the form, though an attribution in a comment
# is refused all the same. What a --cleanup=whitespace or verbatim commit keeps
# of them is then checked literally by --range or --message.
#
# Plain sh and git, so it runs the same from Git for Windows' sh and on Linux.

LC_ALL=C
export LC_ALL

robot=$(printf '\360\237\244\226')
types='feat|fix|docs|chore|refactor|test|build|ci|perf|style'

# Characters, not bytes: with LC_ALL=C a shell counts bytes, so the UTF-8
# continuation bytes go first and one byte per character stays.
length() { printf '%s' "$1" | tr -d '\200-\277' | wc -c | tr -d ' '; }

matches() { printf '%s\n' "$1" | grep -qE "$2"; }
matches_i() { printf '%s\n' "$1" | grep -qiE "$2"; }

cr=$(printf '\r')
scissors=' ------------------------ >8 ------------------------'

# What opens a comment line in the hook's message file: core.commentChar and
# core.commentString are one setting under two names, and as in git the one
# configured last wins ("#" when neither is). With "auto", git picks per message
# the first of #;@!$%^&|: that opens no line of the text, and tells nobody; a
# guess read off the file would let a message choose its own comment character
# and hide a trailer behind a scissors line of it, so nothing is a comment
# then, and the author is told why a comment line of theirs is being judged.
comment_prefix() {
    c=$(git config --get-regexp '^core\.comment(char|string)$' 2>/dev/null | tail -n 1)
    case $c in
        *' '*) c=${c#* } ;;
        *) c= ;;
    esac
    [ -n "$c" ] || c='#'
    if [ "$c" = auto ]; then
        echo 'check-commit-msg: core.commentChar is auto, so comment lines and a scissors line are checked as text' >&2
        return
    fi
    printf '%s' "$c"
}

# Checks the message on stdin, printing "line N (rule): text" per problem, and
# fails on one. $1 is hook or stored: the hook skips comment lines (opened by
# $comment, when set) and stops at the scissors line. A stored message is
# read as it is. $2 says whether to check the form as well as attribution.
check_message() {
    mode=$1
    form=$2
    n=0
    subject=
    k=0
    bad=0
    while IFS= read -r line || [ -n "$line" ]; do
        n=$((n + 1))
        line=${line%"$cr"} # a CRLF file: the CR is no part of the line
        if [ "$mode" = hook ] && [ -n "$comment" ] && [ "$line" = "$comment$scissors" ]; then
            break
        fi
        why=
        case $line in
            *"$robot"*) why='robot emoji' ;;
        esac
        if [ -z "$why" ]; then
            if matches_i "$line" '^[[:space:]]*co-authored-by[[:space:]]*:'; then
                why='Co-Authored-By trailer'
            elif matches "$line" '^[[:space:]#]*Generated with ([A-Z[]|https?://)'; then
                why='"Generated with" attribution'
            fi
        fi
        if [ -n "$why" ]; then
            printf '  line %s (AI attribution, %s): %s\n' "$n" "$why" "$line"
            bad=1
        fi

        [ "$form" = yes ] || continue
        if [ "$mode" = hook ] && [ -n "$comment" ]; then
            case $line in "$comment"*) continue ;; esac
        fi
        case $line in
            *[![:space:]]*) blank=no ;;
            *) blank=yes ;;
        esac
        if [ -z "$subject" ]; then
            [ "$blank" = no ] || continue # leading blank lines go with the commit
            subject=$line
            case $subject in
                'Merge '* | 'Revert "'*) form=no ;;
                'fixup! '* | 'squash! '* | 'amend! '*)
                    printf '  line %s (fixup!/squash!/amend! commit, squash it before merging): %s\n' "$n" "$line"
                    bad=1
                    form=no
                    ;;
            esac
            if matches "$subject" '^Luna [0-9]+\.[0-9]+\.[0-9]+$'; then form=no; fi
            [ "$form" = yes ] || continue
            if [ "$(length "$subject")" -gt 72 ]; then
                printf '  line %s (subject longer than 72 characters, %s): %s\n' "$n" "$(length "$subject")" "$line"
                bad=1
            fi
            case $subject in
                *.)
                    printf '  line %s (subject ends in a period): %s\n' "$n" "$line"
                    bad=1
                    ;;
            esac
            if matches_i "$subject" "^($types)(\([^)]*\))?!?: "; then
                printf '  line %s (type prefix; say what the change does, the type is in the sentence): %s\n' "$n" "$line"
                bad=1
            elif ! matches "$subject" '^([A-Z0-9]|\.?[a-z][a-z0-9]*[-_./][^ ]*)'; then
                printf '  line %s (subject starts with a lowercase word; start with a capital, a digit or a path-like name): %s\n' "$n" "$line"
                bad=1
            fi
            continue
        fi
        k=$((k + 1))
        if [ "$k" = 1 ] && [ "$blank" = no ]; then
            printf '  line %s (line 2 is not blank; a blank line separates the subject from the body): %s\n' "$n" "$line"
            bad=1
        fi
        if [ "$(length "$line")" -gt 72 ]; then
            case $line in
                *://* | [[:space:]]*) ;; # a URL, an indented line
                *' '*)
                    printf '  line %s (body line longer than 72 characters, %s; wrap it): %s\n' "$n" "$(length "$line")" "$line"
                    bad=1
                    ;;
            esac
        fi
    done
    return $bad
}

refuse() { # $1 = what was checked, $2 = the problems, $3 = the fix, $4 = check form
    {
        echo "commit message refused ($1):"
        echo "$2"
        echo
        if [ "$4" = yes ]; then
            echo 'The rules are in CONTRIBUTING.md, "Commits": a sentence of at most 72'
            echo 'characters saying what the change does, a blank line, then why, wrapped at'
            echo '72; no type prefix, no trailing period, no AI attribution.'
        else
            echo 'The rules are in CONTRIBUTING.md, "Commits": no AI attribution.'
        fi
        echo "$3"
    } >&2
}

usage() {
    echo 'usage: check-commit-msg.sh [--branch NAME] (FILE | --range REVS...) | --message FILE' >&2
    exit 2
}

branch_form=yes
branch_given=no
if [ "$1" = --branch ]; then
    [ "$#" -ge 3 ] || usage
    branch_given=yes
    case $2 in
        '' | master | iteration/* | release/*) ;;
        *) branch_form=no ;;
    esac
    shift 2
fi

case $1 in
    --range)
        shift
        [ "$#" -gt 0 ] || usage
        # rev-list has said what is wrong with the revisions; a bad range is
        # not an empty one.
        revs=$(git rev-list "$@") || exit 2
        bad=0
        for c in $revs; do
            # A merge's message is git's own: it is not held to the form,
            # only to the attribution.
            form=$branch_form
            case $(git show -s --format=%P "$c") in *' '*) form=no ;; esac
            if ! out=$(git show -s --format=%B "$c" | check_message stored "$form"); then
                refuse "commit $(git rev-parse --short "$c")" "$out" 'Rewrite the commit message before merging.' "$form"
                bad=1
            fi
        done
        exit $bad
        ;;
    --message)
        [ "$branch_given" = no ] && [ "$#" -eq 2 ] || usage
        if ! out=$(check_message stored yes <"$2"); then
            refuse 'stored message' "$out" 'Edit the message before merging.' yes
            exit 1
        fi
        ;;
    '' | --*) usage ;;
    *)
        [ "$#" -eq 1 ] || usage
        comment=$(comment_prefix)
        if ! out=$(check_message hook "$branch_form" <"$1"); then
            # The refused commit was never made and the change is still staged, so
            # the way on is a new commit from the message; --amend would fold the
            # change into the commit before, unless that was what was being done.
            refuse 'commit-msg hook' "$out" "The commit was not made. Fix the message and commit again: git commit -e -F $1
(--amend only if the refused commit was an amend)." "$branch_form"
            exit 1
        fi
        ;;
esac
