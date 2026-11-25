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
        # Remove a Windows drive prefix before splitting source and target.
        sub(/^[a-zA-Z]:[/\\]/, "/", spec)
        if (!match(spec, /:/)) return 0
        source = substr(spec, 1, RSTART - 1)
        target = substr(spec, RSTART + 1)
        return target ~ /^([a-zA-Z]:)?[/\\]/ && (source ~ /[/\\]/ || source ~ /^[.$~\140]/)
    }
    function argument(word) {
        if (!has_command) {
            if (word ~ /^[a-zA-Z_][a-zA-Z_0-9]*=/) {
                if (word ~ /^MSYS_NO_PATHCONV=/) disabled = (word == "MSYS_NO_PATHCONV=1")
            } else if (word != "env" && word != "command") {
                docker = (tolower(word) ~ /(^|[\/\\])docker(\.exe)?$/)
                has_command = 1
            }
        } else if (docker && !disabled) {
            if (mount == "mount" && tolower(word) ~ /(^|,)type=bind(,|$)/) return 1
            if (mount == "volume" && bind_volume(word)) return 1
            mount = ""
            if (word == "-v" || word == "--volume") mount = "volume"
            else if (word == "--mount") mount = "mount"
            else if (word ~ /^--volume=/ && bind_volume(substr(word, 10))) return 1
            else if (word ~ /^-v./ && bind_volume(substr(word, word ~ /^-v=/ ? 4 : 3))) return 1
            else if (tolower(word) ~ /^--mount=([^,]+,)*type=bind(,|$)/) return 1
            else if (word == "--") docker = 0
        }
        return 0
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
                if (shell ~ /^[\n;&|()]/) has_command = docker = disabled = 0
                if (!has_command) mount = ""
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
        escapes["b"] = "\b"; escapes["f"] = "\f"; escapes["n"] = "\n"
        escapes["r"] = "\r"; escapes["t"] = "\t"
        if (peek() != "{") exit
        value("root")
        if (peek() == "" && tool == "Bash" && unsafe(command)) print "block"
    }
' 2>/dev/null) || exit 0

if [ "$decision" = block ]; then
    echo 'Run Docker bind mounts from pwsh, or prefix the Docker command with MSYS_NO_PATHCONV=1 and give the host path with its drive letter (for example, E:/Projects/x:/app).' >&2
    exit 2
fi
