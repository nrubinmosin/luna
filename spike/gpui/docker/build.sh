#!/usr/bin/env bash
# Runs inside the container (build.ps1): /proj is the source tree (the spike
# folder, or the base-vsync worktree), /out the spike folder the outputs go
# to, $CARGO_TARGET_DIR the cargo target dir in the target volume. $1 is "exe"
# or "tests"; $2, if given, the cargo features, and the exe is then copied as
# spike<variant>-<features>.exe; $SPIKE_VARIANT (e.g. -base) names the tree.
set -euo pipefail
target=x86_64-pc-windows-msvc
out=/out
variant=${SPIKE_VARIANT:-}
target_dir=${CARGO_TARGET_DIR:-/target}
features=(); suffix=$variant
if [ -n "${2:-}" ]; then features=(--features "$2"); suffix="$variant-${2//,/-}"; fi
rustc -V
cargo -V
case "${1:-exe}" in
  exe)
    start=$(date +%s)
    cargo xwin build --release --locked --target "$target" "${features[@]}"
    echo "cargo xwin build --release: $(( $(date +%s) - start )) s"
    cp "$target_dir/$target/release/spike.exe" "$out/spike$suffix.exe"
    ls -l "$out/spike$suffix.exe"
    ;;
  tests)
    # The test executables, built but not run (they run on Windows, not here),
    # copied to tests<variant>/ under their crate's name.
    start=$(date +%s)
    # json-render-diagnostics: compiler messages are rendered to stderr as
    # usual; only cargo's own JSON (the executables) goes down the pipe.
    cargo xwin test --release --locked --no-run --target "$target" "${features[@]}" --message-format=json-render-diagnostics \
      | grep -o '"executable":"[^"]*"' | sed 's/^"executable":"//; s/"$//' > "$target_dir/test-exes.txt"
    echo "cargo xwin test --no-run: $(( $(date +%s) - start )) s"
    tests="tests$variant"
    rm -rf "$out/$tests" && mkdir -p "$out/$tests"
    while read -r exe; do
      name=$(basename "$exe")
      cp "$exe" "$out/$tests/${name%-*}.exe"
      echo "$tests/${name%-*}.exe <- $exe"
    done < "$target_dir/test-exes.txt"
    ;;
  *) echo "unknown mode $1" >&2; exit 2 ;;
esac
