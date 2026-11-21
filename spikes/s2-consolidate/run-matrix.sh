#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"

out="${1:-results}"
max_calls="${MAX_CALLS:-700}"
cargo build --locked
binary=(cargo run --locked --quiet --)
common=(--backend claude-cli --max-calls "$max_calls")
wide=(--window 12/90 --short-only --two-stage-only --repeats 1)

echo 'Call estimates for the three runs'
"${binary[@]}" estimate "${common[@]}" --model sonnet
"${binary[@]}" estimate "${common[@]}" --model haiku
"${binary[@]}" estimate "${common[@]}" --model sonnet "${wide[@]}"

for name in sonnet haiku sonnet-wide; do
    if [[ -e "$out/$name" || -L "$out/$name" ]]; then
        echo "Output already exists: $out/$name" >&2
        exit 1
    fi
done

run_and_report() {
    local destination=$1
    shift
    local status=0
    "${binary[@]}" run "${common[@]}" --out "$destination" "$@" || status=$?
    if [[ -f "$destination/config.json" ]]; then
        "${binary[@]}" report --out "$destination" || status=$?
    fi
    return "$status"
}

status=0
run_and_report "$out/sonnet" --model sonnet || status=$?
run_and_report "$out/haiku" --model haiku || status=$?
run_and_report "$out/sonnet-wide" --model sonnet "${wide[@]}" || status=$?
exit "$status"
