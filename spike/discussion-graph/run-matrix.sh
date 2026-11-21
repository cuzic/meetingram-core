#!/usr/bin/env bash
# Spike S1: run the extraction matrix in parallel, then evaluate. Usage: ./run-matrix.sh
set -u
cd "$(dirname "$0")"
run() { # name meeting backend model extra...
  local name=$1 m=$2 b=$3 model=$4; shift 4
  python3 -I harness.py run --meeting meetings/$m.json --backend $b --model "$model" --out results/$name.json "$@" 2> results/$name.log
}
for m in m1 m2 m3; do
  run $m-claude-sonnet $m claude sonnet &
  run $m-claude-haiku  $m claude haiku &
done
run m1-codex m1 codex "" &
run m1-claudelean-haiku m1 claude-lean haiku &
run m1-claude-haiku-b12 m1 claude haiku --batch 12 --secs 90 &
wait
for f in results/*.json; do
  n=$(basename "$f" .json); m=${n%%-*}
  python3 -I harness.py eval --result "$f" --gold gold/$m.gold.json > results/$n.eval.txt
done
echo done
