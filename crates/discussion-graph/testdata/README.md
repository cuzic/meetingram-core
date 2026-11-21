# 合成データ

すべて合成した会議と手作業で付けた正解。発話は JSONL、正解は JSON で、根拠は発話の `row_id` を指す。

| データ | ブランチ | 発話と正解の出典 |
| --- | --- | --- |
| m1、m2、m3 | `spike/discussion-graph-s1-measure` | `spike/discussion-graph/meetings/*.json` と `spike/discussion-graph/gold/*.gold.json` |
| budget、design、one-on-one | `spike/discussion-graph` | `spikes/s1-discussion-graph/data/*.jsonl` と `spikes/s1-discussion-graph/gold/*.json` |

出典で状態の注釈が省略されたノードには、ADR の初期状態 `open` を入れた。
