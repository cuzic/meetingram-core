# S1 の実測（2026-11-21、Claude が sandbox の外で実行）

- `RESULTS-lean.md`: `./run-matrix.sh live <会議>` を 3 本並行で実行（378 回、cli-mode=lean）。claude 2.1.330、codex-cli 0.167.0。
- `RESULTS-full-partial.md`: 今の呼び方（full）で sonnet の基本設定だけ回して止めた分（84 回）。
- 生の calls.jsonl と graph.json はコミットしていない（合成データだが量が多い）。再現は README の手順で。
- 予測ノード数（merged を除く）と正解のノード数:

| 会議 | 正解 | sonnet 6/45 K3 | sonnet 12/90 | sonnet K10 | haiku | codex |
| --- | --- | --- | --- | --- | --- | --- |
| design | 39 | 121 | 89 | 116 | 135 | 170 |
| budget | 42 | 98 | 95 | 106 | 140 | 168 |
| one-on-one | 42 | 114 | 92 | 117 | 137 | 163 |

issue は正解 6〜7 に対して予測 16〜37。会議中の `merge` は 1 実行あたり 0〜7 回。
