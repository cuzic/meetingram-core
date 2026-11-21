# PR #15 レビュー（round 2）

対象: cuzic/meetingram-core feat/discussion-graph-store @ e12d271a97c1cc60faededad35b9808cfa360792

## 結論（先に）
converged: yes（Blocker 0 / Major 0）。round 1 の M1・m1〜m4・n2 はすべて、round 1 の変異を当て直すとテストが落ちることで解消を確かめた。
新しい差分（`993a878..e12d271`）の中心である m3 の「0 件のパッチにも rev を振る」経路は、`BEGIN IMMEDIATE` の中で `MAX(rev) + 1` を読んで振るので、rev の単調増加と一意性を保つ。新しい Blocker・Major は無い。nit が 1 つ。

## 前ラウンドの解消状況
- M1: 解消。`persists_merge_status_redirect_and_grounds` が `merge`（`merged`・`merged_into`）、`set_status`、エッジの付け替えと重複の削除、根拠の移動を開き直した接続で確かめる。ノードの upsert から `status = excluded.status` を外す変異と、根拠の削除を飛ばす変異の両方で落ちる（再現した）。
- m1: 解消。`imports_batch_in_transcript_row_order` が 3 行の一括取り込みを確かめ、`ORDER BY id DESC` で落ちる。
- m2: 解消。`AND extracted_rev IS NULL` を外すと `patches_consume_only_input…` が落ち（`extracted_rev` が `[1, 4]` でなくなる）、ノード・エッジそれぞれの `AND review = 'auto'` を外すと `cannot_review_a_decided_node_or_edge_again` が落ちる。
- m3: 解消（決定は「0 件でも `graph_patches` に 1 行足して rev を進め、`extracted_rev` はその rev、`rejected_ops` を残す」）。変異（0 件のとき行を足さない／`rejected_ops` を 0 で書く／`extracted_rev` に前の rev を入れる）のどれでも `patches_consume_only_input…` が落ちる。
- m4: 解消。`Utterance.is_retranscribed`（`#[serde(default)]`）を `transcript_segments` から JOIN で返す。常に false にする変異で `imports_batch_in_transcript_row_order` が落ちる。
- n2: 解消。`COALESCE(t.text, '')` で、行が無くても `load_graph` が失敗しない。`COALESCE` を外すと `loads_utterance_without_transcript_row` が落ちる。
- n1（話者ラベルの `u32::MAX`）: 対応なしでよい（round 1 でもそのままでよいとした）。

## 確認したこと
- AGENTS.md の完了の定義（Linux）: `cargo fmt --all --check`、workspace の clippy、`cargo test --workspace …`（全 pass、失敗 0）、`capture-windows` の Windows 向け clippy、ignored の `stored_budget_meeting_matches_gold_graph` → すべて pass。`gh pr checks 15` は 5/5 pass（Linux・Windows・macOS の build、Windows・macOS の e2e-best-effort）。
- rev の振り方（`store/mod.rs` の `apply_patch`）: `before` は `begin_write()`（Immediate）の後に `load_graph(&tx)` で読むので、`before.rev = MAX(rev)` はロックの中の値。`rev = before.rev + 1` で `graph_patches` に入れ、主キー `(session_id, rev)` が一意性を保証する。`set_review` も同じトランザクションの中で `MAX + 1`。1 件以上適用したときは、メモリ上の `apply_patch` が既に `before.rev + 1` にしていて、ノードの `updated_rev` と一致する。0 件のときは `write_changes` を飛ばし、ノードの変更は無い（`touched` は成功した操作でしか増えない）。
- ロックの中の追加の処理は、空の配列の直列化と 1 行の INSERT だけ。`BEGIN IMMEDIATE` を Deferred にする変異では、引き続き `recording_writer_waits_for_graph_write_transaction` が落ちる。
- 公開 API の変更（`Utterance` の列の追加、0 件のパッチでも rev が進むこと）は PR 本文に書かれている。

## Findings
### n1（nit）: 保存の後の `PatchResult::patch_rev()` が、保存した rev と食い違う
- 根拠: 0 件のパッチでも保存の層は rev を振って `result.graph.rev` に入れるが、`patch_rev()`（`patch.rs:125`）は `applied` が空なら `None` を返す。`GraphStore::apply_patch` の doc は「`result.graph.rev` と `applied.is_empty()` を使え」と書いているので、呼び出し側が doc どおりなら問題は無い。
- 修正案: タスク 5 で `graph.{sid}.updated` の rev を取るときは `result.graph.rev` を使う（必要なら `patch_rev()` の doc に「メモリ上の適用での値。保存した rev は `graph.rev`」と添える）。

## 判定
converged: yes（Blocker 0 / Major 0 / Minor 0 / nit 1）
