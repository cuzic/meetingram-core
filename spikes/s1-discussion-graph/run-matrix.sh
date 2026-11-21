#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
mode=${1:-live}
case "$mode" in live|fake) ;; *) echo "usage: $0 [live|fake] [meeting] [output-dir]" >&2; exit 2;; esac
meetings=(design budget one-on-one)
if [[ -n ${2:-} ]]; then
  case "$2" in design|budget|one-on-one) meetings=("$2");; *) exit 2;; esac
fi
out=${3:-runs/$(date -u +%Y%m%dT%H%M%SZ)}
mkdir -p "$out"
cargo build --locked --bin s1-discussion-graph
bin="${CARGO_TARGET_DIR:-target}/debug/s1-discussion-graph"
limit=400
budget=$(mktemp)
trap 'rm -f "$budget"' EXIT
echo "$limit" > "$budget"
settings=(
  'claude sonnet 6 45 3'
  'claude haiku 6 45 3'
  'codex default 6 45 3'
  'claude sonnet 12 90 3'
  'claude sonnet 6 45 10'
)
total=0
for setting in "${settings[@]}"; do
  read -r backend model count seconds k <<< "$setting"
  for meeting in "${meetings[@]}"; do
    calls=$("$bin" count --input "data/$meeting.jsonl" --count "$count" --seconds "$seconds")
    total=$((total + calls))
  done
done
echo "基本呼び出し $total 回（再試行・入力分割を含めて最大 $limit 回）"
if ((total > limit)); then exit 2; fi
runs=()
for setting in "${settings[@]}"; do
  read -r backend model count seconds k <<< "$setting"
  for meeting in "${meetings[@]}"; do
    dir="$out/$meeting-$backend-$model-${count}n-${seconds}s-${k}k"
    runs+=("$dir")
    model_args=()
    if [[ "$mode" == fake ]]; then
      backend_arg=fake
    else
      backend_arg=$backend
      if [[ "$model" != default ]]; then model_args=(--model "$model"); fi
    fi
    "$bin" run --input "data/$meeting.jsonl" --gold "gold/$meeting.json" \
      --backend "$backend_arg" "${model_args[@]}" --count "$count" --seconds "$seconds" \
      --k "$k" --call-budget "$budget" --out "$dir"
  done
done
"$bin" aggregate --out "$out/RESULTS.md" "${runs[@]}"
