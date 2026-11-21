# PR #12 レビュー（round 2）

対象: cuzic/meetingram-core feat/discussion-graph-core @ b9f4b8b34bcf46918a8ea60471cbd5bed9e3f3db（round 1 の 900be46 からの差分を見た）

## 結論（先に）
converged: yes。round 1 の M1・M2・m1・m2・m3・n1 は、挙動とテストで解消を確かめた。各修正の検査を外す変異では、どれも新しいテストか直したテストが落ちた。
差分に新しい Blocker / Major は無い。Minor が 1 件ある。Codex が足した「人の判断済みのエッジを残す」分岐に、テストが無い。
DoD の 4 コマンドは手元で成功し、`gh pr checks 12` も 5 件すべて pass。

## 前ラウンドの解消状況
- M1（文脈の落とし方）: 解消。`updated_rev`（0 なら `created_rev`）の古い順に落とし、元の並びに戻すようになった（`src/prompt.rs:56-75`・`:134-135`）。
  テストは、更新の新しい open の issue n1 と、直近で触った n6 が残る形に直った。並べ替えを消す変異でテストが落ちる。
- M2（誤った merge）: 解消。整理の前のグラフでは、各ノードを Jaccard が最大の正解ノードに排他なしで写すようになった（`src/eval.rs:71-74`。候補は Jaccard の降順なので、最初に入るものが最大）。
  round 1 のプローブと同じ形のテスト `merging_duplicates_into_the_node_with_more_grounds_is_not_an_error` が足された。1 対 1 の `matching` に戻す変異で、このテストが落ちる。
- m1（tmp が既存の nid を隠す）: 解消。既存の nid と同じ `tmp` の `add_node` は `TmpMatchesNodeId` で落ちる。検査を消す変異で `rejects_tmp_matching_existing_node_id` が落ちる。
- m2（`seen_tmp` のテストの穴）: 解消。既存の nid と同名の落ちた tmp を指す後続（edge・status・merge）が落ちるテストが足された。`seen_tmp` の検査を消す変異で、このテストが落ちる。
  落ちた tmp と同名の既存ノードへの参照も落とすという判断は、ADR の連鎖の行の保守的な読みとして妥当。
- m3（空のパッチで rev が進む）: 解消。適用が 0 件なら rev もグラフも変わらず、`PatchResult::patch_rev()` が `None` を返す。新しいノードとエッジの `created_rev` は `rev + 1` で、最後に rev を進めるので値が一致する。
  touched は成功した操作でしか増えないので、`updated_rev` も一致する。常に rev を進める変異で `keeps_revision_when_no_operations_apply` が落ちる。
- n1（付け替えで重複したエッジの残し方）: 解消。`created_rev`、eid の数値部分の小さい方を残すようになった（`edge_age`）。`redirect_prefers_earlier_creation_then_numeric_edge_id` があり、比較を外す変異でテストが落ちる。
  `self.graph.edges[existing_id]` の添字は、`keys` が常に存在するエッジだけを指すので（置き換えたら `keys` も更新する）panic しない。
- n2（`Op` が型を持たない）: 仕様どおり対応なし。

## Findings
### m1（Minor）: 付け替えで「人の判断済みのエッジを残す」分岐にテストが無い
- 根拠: `src/patch.rs:449-452` の `existing.review != Review::Auto ||` を消しても、`discussion-graph` の全テストが通る（変異で確認）。
  消すと、`into` 側に新しく承認されたエッジがあり、`from` 側に同じ `(src, dst, kind)` で古い auto のエッジがある場合に、承認されたエッジが消える（ADR §2「人の判断はノードもエッジも守る」）。今の挙動は正しい。
- 修正案: `into` 側に `created_rev` の新しい approved のエッジを置き、`from` 側の古い auto のエッジと重なる `merge` で、approved のエッジとその根拠が残るテストを足す（follow-up でよい）。

## 判定
converged: yes（Blocker 0 / Major 0 / Minor 1 / nit 0）
