#!/bin/sh
# Test the hook protocol without starting Claude Code or Docker.
here=$(cd "$(dirname "$0")" && pwd)
fail=0

check() { # $1 = expected exit, $2 = uname -s, $3 = hook JSON
    output=$(printf '%s\n' "$3" | LUNA_HOOK_UNAME="$2" sh "$here/docker-msys.sh" 2>&1)
    status=$?
    if [ "$status" -ne "$1" ]; then
        printf 'FAIL (%s): expected %s, got %s for %s\n%s\n' "$2" "$1" "$status" "$3" "$output" >&2
        fail=1
    elif [ "$status" -eq 2 ]; then
        case $output in
            *pwsh*MSYS_NO_PATHCONV=1*MSYS2_ARG_CONV_EXCL*drive\ letter*//*) ;;
            *) printf 'FAIL: refusal did not list the conversion workarounds: %s\n' "$output" >&2; fail=1 ;;
        esac
    fi
}

bash_command() { # $1 = expected exit, $2 = shell command escaped as JSON string content
    check "$1" MINGW64_NT-10.0 "{\"tool_name\":\"Bash\",\"tool_input\":{\"command\":\"$2\"}}"
}

bind='{"tool_name":"Bash","tool_input":{"command":"docker run -v /e/Projects/x:/app alpine"}}'
check 2 MINGW64_NT-10.0 "$bind"
check 2 MSYS_NT-10.0 "$bind"
check 0 Linux "$bind"
check 0 Darwin "$bind"
bash_command 2 'docker run --volume=\"E:/Project space:/app\" alpine'
bash_command 2 'docker run --mount target=/app,type=bind,src=/e/Projects/x alpine'
bash_command 2 'docker run --mount=\"type=bind,src=/e/Projects/x,dst=/app\" alpine'
bash_command 2 'docker run -v\"$(pwd):/app\" alpine'
bash_command 2 'docker run --volume \"${WORKSPACE}:/app\" alpine'
bash_command 2 'cd /e/Projects && docker.exe run \\\n -v ./x:/app alpine'
bash_command 2 'docker run \u002dv /e/Projects/x:/app alpine'
bash_command 2 '\u0064ocker run -v /e/Projects/x:/app alpine'
bash_command 2 'if true; then docker run -v /x:/y alpine; fi'
bash_command 2 'if false; then :; else docker run -v /x:/y alpine; fi'
bash_command 2 'if false; then :; elif docker run -v /x:/y alpine; then :; fi'
bash_command 2 'for image in alpine; do docker run -v /x:/y \"$image\"; done'
bash_command 2 '{ docker run -v /x:/y alpine; }'
bash_command 2 '! docker run -v /x:/y alpine'
for prefix in winpty exec sudo nohup time; do
    bash_command 2 "$prefix docker run -v /x:/y alpine"
done
bash_command 2 'MSYS_NO_PATHCONV=0 docker run -v /e/x:/app alpine'
bash_command 2 'MSYS_NO_PATHCONV=1 docker ps; docker run -v /e/x:/app alpine'
bash_command 2 'docker run -e MSYS_NO_PATHCONV=1 -v /e/x:/app alpine'
bash_command 2 'echo MSYS_NO_PATHCONV=1; docker run -v /e/x:/app alpine'
bash_command 2 'export MSYS_NO_PATHCONV=1; unset MSYS_NO_PATHCONV; docker run -v /x:/y alpine'
bash_command 2 'export MSYS_NO_PATHCONV=1; MSYS_NO_PATHCONV=0 docker run -v /x:/y alpine'
bash_command 2 'MSYS2_ARG_CONV_EXCL=--label= docker run -v /x:/y alpine'
bash_command 2 'MSYS2_ARG_CONV_EXCL= docker run -v /x:/y alpine'
bash_command 2 'docker run -v //host/share:/app -v /x:/y alpine'
bash_command 0 'MSYS_NO_PATHCONV=1 docker run -v E:/Projects/x:/app alpine'
bash_command 0 'env MSYS_NO_PATHCONV=\"1\" docker run -v \"E:/Project space:/app\" alpine'
bash_command 0 'export MSYS_NO_PATHCONV=1; cd /e/Projects && docker run -v E:/x:/app alpine'
bash_command 0 'MSYS_NO_PATHCONV=1; export MSYS_NO_PATHCONV; docker run -v E:/x:/app alpine'
bash_command 0 'MSYS2_ARG_CONV_EXCL=\"*\" docker run -v E:/x:/app alpine'
bash_command 0 'export MSYS2_ARG_CONV_EXCL=\"*\"; docker run -v E:/x:/app alpine'
bash_command 0 'MSYS2_ARG_CONV_EXCL=\"--label=;--volume=\" docker run --volume=/x:/y alpine'
bash_command 0 'MSYS2_ARG_CONV_EXCL=/x: docker run -v /x:/y alpine'
bash_command 0 'docker run -v //host/share:/app alpine'
bash_command 0 'docker run --volume=//host/share:/app alpine'
bash_command 0 'docker run --mount=\"type=bind,source=//host/share,target=/app\" alpine'
bash_command 0 'docker version'
bash_command 0 'docker run -v cache:/app -v /scratch --mount source=logs,target=/logs alpine'
bash_command 0 'docker run --mount=type=volume,source=cache,target=/app alpine'
bash_command 0 'echo \"docker run -v /e/x:/app alpine\" # docker run -v /e/x:/app alpine'
bash_command 0 'echo docker run -v /e/x:/app alpine'
check 0 MSYS_NT-10.0 '{"tool_name":"Other","tool_input":{"command":"docker run -v /e/x:/app alpine"}}'
check 0 MSYS_NT-10.0 '{"tool_name":"Bash","tool_input":{"nested":{"command":"docker run -v /e/x:/app alpine"}}}'
check 0 MSYS_NT-10.0 "$bind trailing"
check 0 MSYS_NT-10.0 '{"tool_name":"Bash","tool_input":{"command":"docker run -v /e/x:/app alpine",}}'
check 0 MSYS_NT-10.0 '{"tool_name":"Bash","tool_input":{"command":"docker run -v \q:/app alpine"}}'
check 0 MSYS_NT-10.0 ''
large_command=$(awk 'BEGIN { printf "echo"; for (i = 0; i < 115000; i++) printf " x" }')
bash_command 0 "$large_command"
check 2 MSYS_NT-10.0 '{
    "tool_input": {"command": "docker run -v \"/e/Project space:/app\" alpine"},
    "other": [true, {"tool_name": "Other"}, null, 1e-4],
    "tool_name": "Bash"
}'

[ "$fail" -eq 0 ] && echo 'docker-msys: all cases passed'
exit "$fail"
