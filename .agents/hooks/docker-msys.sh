#!/bin/sh
# Claude Code PreToolUse guard for path rewriting in Git Bash on Windows.
# LUNA_HOOK_UNAME substitutes for uname -s in the tests.
case ${LUNA_HOOK_UNAME:-$(uname -s)} in
    MINGW* | MSYS*) ;;
    *) exit 0 ;;
esac

# Read the JSON as data so neither shell code nor JSON escapes can be mistaken
# for another field. Only tool_name and tool_input.command affect the decision.
decision=$(LC_ALL=C awk '
    function take(n,    head) {
        head = substr(json, 1, n)
        json = substr(json, n + 1)
        return head
    }
    function peek() {
        sub(/^[ \t\r\n]+/, "", json)
        return substr(json, 1, 1)
    }
    function string_value(    c, digits, i, n, raw, result) {
        if (!match(json, /^"([^"\\[:cntrl:]]|\\(["\\/bfnrt]|u[0-9a-fA-F][0-9a-fA-F][0-9a-fA-F][0-9a-fA-F]))*"/)) exit
        raw = take(RLENGTH)
        raw = substr(raw, 2, length(raw) - 2)
        while (match(raw, /\\/)) {
            result = result substr(raw, 1, RSTART - 1)
            c = substr(raw, RSTART + 1, 1)
            raw = substr(raw, RSTART + 2)
            if (c == "u") {
                digits = tolower(substr(raw, 1, 4))
                raw = substr(raw, 5)
                n = 0
                for (i = 1; i <= 4; i++)
                    n = n * 16 + index("0123456789abcdef", substr(digits, i, 1)) - 1
                # Only ASCII syntax matters to the command scan.
                c = n > 0 && n < 128 ? sprintf("%c", n) : "?"
            } else if (c in escapes) {
                c = escapes[c]
            }
            result = result c
        }
        return result raw
    }
    function value(scope,    c, text) {
        c = peek()
        if (c == "{") group("}", scope)
        else if (c == "[") group("]", "")
        else if (c == "\"") {
            text = string_value()
            if (scope == "tool_name") tool = text
            if (scope == "command") command = text
        } else if (match(json, /^(null|true|false|-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?)/)) take(RLENGTH)
        else exit
    }
    function group(end, scope,    child, key) {
        take(1)
        if (peek() != end) {
            do {
                child = ""
                if (end == "}") {
                    peek()
                    key = string_value()
                    if (peek() != ":") exit
                    take(1)
                    if (scope == "root" && (key == "tool_name" || key == "tool_input"))
                        child = key
                    if (scope == "tool_input" && key == "command") child = key
                }
                value(child)
                if (peek() != ",") break
                take(1)
            } while (1)
        }
        if (peek() != end) exit
        take(1)
    }

    function bind_volume(spec,    source, target) {
        if (spec ~ /^\/\//) return 0
        # Remove a Windows drive prefix before splitting source and target.
        sub(/^[a-zA-Z]:[/\\]/, "/", spec)
        if (!match(spec, /:/)) return 0
        source = substr(spec, 1, RSTART - 1)
        target = substr(spec, RSTART + 1)
        return target ~ /^([a-zA-Z]:)?[/\\]/ && (source ~ /[/\\]/ || source ~ /^[.$~\140]/)
    }
    function save_assignment(word, values,    eq, name) {
        eq = index(word, "=")
        name = substr(word, 1, eq - 1)
        if (name == "MSYS_NO_PATHCONV" || name == "MSYS2_ARG_CONV_EXCL")
            values[name] = substr(word, eq + 1)
    }
    function conversion_setting(name) {
        if (name in inline) return inline[name]
        if (exported[name]) return shell_values[name]
        return ""
    }
    function rewrites_mount(kind, spec, arg,    value, prefixes, count, i) {
        if (kind == "volume" && !bind_volume(spec)) return 0
        if (kind == "mount" && (tolower(spec) !~ /(^|,)type=bind(,|$)/ ||
                                tolower(spec) ~ /(^|,)(source|src)=\/\//)) return 0
        if (conversion_setting("MSYS_NO_PATHCONV") == "1") return 0
        value = conversion_setting("MSYS2_ARG_CONV_EXCL")
        if (value == "*") return 0
        count = split(value, prefixes, ";")
        for (i = 1; i <= count; i++)
            if (prefixes[i] != "" && index(arg, prefixes[i]) == 1) return 0
        return 1
    }
    function argument(word,    name, kind, spec) {
        if (!has_command) {
            if (word ~ /^[a-zA-Z_][a-zA-Z_0-9]*=/) {
                save_assignment(word, inline)
            } else if (word ~ /^(if|while|until|then|do|else|elif|[{]|!|time)$/) {
                return 0
            } else if (word ~ /^(env|command|winpty|exec|sudo|nohup|--)$/) {
                wrapped = 1
            } else {
                shell_builtin = word
                docker = (tolower(word) ~ /(^|[\/\\])docker(\.exe)?$/)
                has_command = 1
            }
        } else if (!wrapped && shell_builtin == "export" && word ~ /^(MSYS_NO_PATHCONV|MSYS2_ARG_CONV_EXCL)(=|$)/) {
            name = word
            sub(/=.*/, "", name)
            if (index(word, "=")) save_assignment(word, shell_values)
            exported[name] = 1
        } else if (!wrapped && shell_builtin == "unset" && word ~ /^(MSYS_NO_PATHCONV|MSYS2_ARG_CONV_EXCL)$/) {
            delete shell_values[word]
            delete exported[word]
        } else if (docker) {
            kind = mount
            mount = ""
            if (kind != "") return rewrites_mount(kind, word, word)
            if (word == "-v" || word == "--volume") mount = "volume"
            else if (word == "--mount") mount = "mount"
            else if (word ~ /^--volume=/) return rewrites_mount("volume", substr(word, 10), word)
            else if (word ~ /^-v./) {
                spec = substr(word, word ~ /^-v=/ ? 4 : 3)
                return rewrites_mount("volume", spec, word)
            } else if (word ~ /^--mount=/) return rewrites_mount("mount", substr(word, 9), word)
            else if (word == "--") docker = 0
        }
        return 0
    }
    function end_command(    name) {
        # A standalone assignment can be exported later. A prefix on a
        # command lasts only as long as that command.
        for (name in inline) {
            if (!has_command && !wrapped) shell_values[name] = inline[name]
            delete inline[name]
        }
        has_command = docker = wrapped = 0
        mount = shell_builtin = ""
    }

    # Inspect direct commands and their arguments without expanding variables
    # or running shell code. Keep quoted paths and simple substitutions whole.
    function unsafe(shell,    part, word, started) {
        while (length(shell)) {
            if (sub(/^\\\n/, "", shell)) continue
            if (!started && sub(/^#[^\n]*/, "", shell)) continue
            if (shell ~ /^[ \t\r\n;&|()]/) {
                if (started && argument(word)) return 1
                word = ""
                started = 0
                if (shell ~ /^[\n;&|()]/) end_command()
                shell = substr(shell, 2)
                continue
            }
            if (!match(shell, /^(\$\([^()]*\)|\140[^\140]*\140|"([^"\\]|\\.)*"|\047[^\047]*\047|\\[^\n]|[^ \t\r\n;&|()"\047\\$\140]+|[$])/)) return 0
            part = substr(shell, 1, RLENGTH)
            shell = substr(shell, RLENGTH + 1)
            if (part ~ /^["\047]/) part = substr(part, 2, length(part) - 2)
            else if (part ~ /^\\/) part = substr(part, 2)
            word = word part
            started = 1
        }
        return started && argument(word)
    }

    { json = json $0 "\n" }
    END {
        # Most tool calls never mention Docker. Allow JSON Unicode escapes
        # through this first filter since they can spell part of its name.
        if (tolower(json) !~ /docker|\\u00/) exit
        escapes["b"] = "\b"; escapes["f"] = "\f"; escapes["n"] = "\n"
        escapes["r"] = "\r"; escapes["t"] = "\t"
        if (peek() != "{") exit
        value("root")
        if (peek() == "" && tool == "Bash" && tolower(command) ~ /docker/ && unsafe(command)) print "block"
    }
' 2>/dev/null) || exit 0

if [ "$decision" = block ]; then
    printf '%s\n' \
        'Run Docker bind mounts from pwsh.' \
        "In Git Bash, prefix the command with MSYS_NO_PATHCONV=1 or MSYS2_ARG_CONV_EXCL='*' (or export either first) and give the host path with its drive letter." \
        'A host path beginning with // also skips conversion.' >&2
    exit 2
fi
