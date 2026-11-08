#!/bin/sh
# The rules for commit messages (CONTRIBUTING.md, "Commits"), checked once here
# for the commit-msg hook and for CI.
#
# Every commit: authorship is git's author field and nothing else, so a
# message carries no AI attribution. Refused:
#   - a Co-Authored-By: trailer (any case, blanks before the colon allowed),
#   - the robot emoji (U+1F916),
#   - a "Generated with" attribution line: "Generated with " opening a line
#     (after blanks, #) and followed by a capitalized word, a [bracketed link]
#     or a URL. Prose like "generated with drizzle-kit" passes.
#
# The form, a sentence saying what the change does:
#   - the subject is at most 72 characters and does not end in a period,
#   - it starts with a capital letter, a digit or a path-like name
#     ("agents/wait.rs:", "check-text", "luna_wait"); lowercase prose is
#     refused, and so is a Conventional Commits type prefix ("docs:",
#     "fix(scope)!:"): the type is what the sentence says,
#   - line 2 is blank, and the body is wrapped at 72, except a line with a
#     URL, an indented line and a line that is one unbreakable token.
# Exempt from the form: a merge ("Merge ..."), a revert ('Revert "..."') and
# the release bump ("Luna X.Y.Z"), and Dependabot's commits by their author.
# "fixup!" and "squash!" commits pass the hook, to be squashed, and fail the
# range check, so they never merge.
#
#   check-commit-msg.sh FILE            one message file (the commit-msg hook)
#   check-commit-msg.sh --range A..B    every commit in a range (CI)
#
# Plain sh and git, so it runs the same from Git for Windows' sh and on Linux.

LC_ALL=C
export LC_ALL

robot=$(printf '\360\237\244\226')
dependabot='49699333+dependabot[bot]@users.noreply.github.com'
types='feat|fix|docs|chore|refactor|test|build|ci|perf|style'

# Characters, not bytes: with LC_ALL=C a shell counts bytes, so the UTF-8
# continuation bytes go first and one byte per character stays.
length() { printf '%s' "$1" | tr -d '\200-\277' | wc -c | tr -d ' '; }

matches() { printf '%s\n' "$1" | grep -qE "$2"; }
matches_i() { printf '%s\n' "$1" | grep -qiE "$2"; }

# Checks the message on stdin, printing "line N (rule): text" per problem, and
# fails on one. $1 is hook, range or attribution: the hook sees git's comment
# lines, which are no part of the message, and lets a fixup! through; attribution
# skips the form (Dependabot). A verbose commit's scissors line ends the message.
check_message() {
    mode=$1
    form=yes
    [ "$mode" = attribution ] && form=no
    n=0
    subject=
    k=0
    bad=0
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
        if [ "$mode" = hook ]; then
            case $line in '#'*) continue ;; esac
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
                'fixup! '* | 'squash! '*)
                    if [ "$mode" = range ]; then
                        printf '  line %s (fixup!/squash! commit, squash it before merging): %s\n' "$n" "$line"
                        bad=1
                    fi
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

refuse() { # $1 = what was checked, $2 = the problems, $3 = the fix
    {
        echo "commit message refused ($1):"
        echo "$2"
        echo
        echo 'The rules are in CONTRIBUTING.md, "Commits": a sentence of at most 72'
        echo 'characters saying what the change does, a blank line, then why, wrapped at'
        echo "72; no type prefix, no trailing period, no AI attribution. $3"
    } >&2
}

case $1 in
    --range)
        bad=0
        # Merges are git's own messages; Dependabot's keep their own form but
        # carry no attribution either.
        for c in $(git rev-list --no-merges "$2"); do
            if [ "$(git log -1 --format=%ae "$c")" = "$dependabot" ]; then mode=attribution; else mode=range; fi
            if ! out=$(git show -s --format=%B "$c" | check_message $mode); then
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
        if ! out=$(check_message hook <"$1"); then
            refuse 'commit-msg hook' "$out" 'Edit it with: git commit --amend'
            exit 1
        fi
        ;;
esac
