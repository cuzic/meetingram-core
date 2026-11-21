# PR #15 レビュー（round 1）

対象: cuzic/meetingram-core feat/discussion-graph-store @ 993a878a355b8800610d9bc01dfe48effaba06fe

## 結論（先に）
converged: no（Major 1）。2 本目の接続、`BEGIN IMMEDIATE`、1 トランザクションでの適用、取り込みの条件、削除、話者ラベルの共通化は ADR 0001 r10 と Issue #14 のとおりで、録音側を待たせる時間も測って問題ない（3000 発話・500 ノードで適用 1 回 10〜14ms、取り込み 3000 行 14〜22ms）。
ただし、パッチの適用のうち `merge`・`set_status`・エッジの付け替え・根拠の移動が DB に保存されることを確かめるテストが無い。保存の処理を壊しても全テストが通る（下の M1）。

## 前ラウンドの解消状況
round 1 のため無し。PR #12 round 2 の m1（人の判断済みのエッジを残す分岐）は `redirect_preserves_newer_approved_edge_and_its_ground` で解消（PR 本文の変異の結果を信用し、ここでは再実行していない）。

## 確認したこと
- AGENTS.md の完了の定義（Linux）: `cargo fmt --all --check`、workspace の clippy、`cargo test --workspace …`（全 pass）、`capture-windows` の Windows 向け clippy、ignored の `stored_budget_meeting_matches_gold_graph` → すべて pass。`gh pr checks 15` は 5/5 pass。
- ロックを持つ時間: 書くトランザクションはすべて `begin_write()`（Immediate）か `open` の Immediate。トランザクションの中で LLM の呼び出しも応答の解析も無い（`Op` は解析済みの値）。読み出し（`load_graph` / `pending_*` / `revision`）は deferred の読みだけで、書きに上がらない。プローブ（release、3000 発話＋500 ノード）で `apply_patch` 1 回の最大 10.4ms、3000 行の取り込み 13.6ms。
- `recording_writer_waits_for_graph_write_transaction` は `BEGIN IMMEDIATE` の書きのロックを持ったまま、別スレッドの `register_segment` が 200ms 終わらないことと、commit の後に成功することを確かめている。`begin_write` を Deferred に変えると落ちる（再現した）。
- 取り込み: `is_final = 1 AND id > MAX(row_id)`、`id` 順、`uid` は中で `MAX + 1`。3 本の `GraphStore` から同時に取り込みつつ 500 行を書き込むプローブで、重複も抜けも無く `u1..u500` が `row_id` 順に並んだ。session-store は `transcript_segments` を UPDATE も DELETE もしない（append-only）ので、取り込んだ後に `is_final` が変わる行や消える行は無い。
- 原子性: 失敗のテスト（2 件目の `extracted_rev` の更新でトリガーが失敗）で、パッチ・ノード・根拠・`extracted_rev` のどれも残らない。
- トランザクションの中の `unwrap` / `expect` / 添字: `store/` と、トランザクションの中で走る `patch.rs::apply_patch` に無い（`value[key]` と `unreachable!` は `get` と `Err` に置き換え済み）。
- スキーマは ADR §1 のとおり（5 表、主キー、UNIQUE、`session_id` が先頭）。
- 話者ラベル: 3 つのラベルと話者番号を `recorder-domain` のテストで固定。出力が変わるのは `speaker = u32::MAX` だけ（下の n1）。
- ログ出力は無い。エラー文に本文は入らない（`Track` は track の値、`FromSqlConversionFailure` は列番号だけ）。
- 変異: `is_final = 1` を外す → session-store の往復テストが落ちる。0 件のパッチで入力を消費しない → `patches_consume_only_input…` が落ちる。

## Findings
### M1（Major）: `merge`・`set_status`・エッジの付け替え・根拠の移動が保存されることを確かめるテストが無い
- 根拠: 次の 2 つの変異で `cargo test -p discussion-graph -p session-store` が全部通る。
  (1) `store/records.rs:129` の `ON CONFLICT … DO UPDATE SET` から `status = excluded.status` を外す（`merge` の `from` が DB では `open` のまま、`set_status` が DB に残らない）。
  (2) `store/records.rs:193-199` の根拠の削除を外す（`merge` の後も `from` の根拠が DB に残る）。
  どちらも、メモリ上の `PatchResult.graph` と DB の中身が黙ってずれる。次の読み出しはずれた状態から検証するので、会議の後の整理（`merge` が主な役目。ADR §3）の結果が保存されない。
  ストアのテストで `load_graph == result.graph` を確かめているのは `add_node` だけ（`tests.rs:180`）。差分の書き込み（`write_changes`）の分岐の大半が通っていない。
  今の実装は正しい（`merge`＋エッジ 2 本の付け替え＋`set_status` を適用して `load_graph` と `result.graph` を比べるプローブは一致した）。
- 修正案: `merge`（エッジの付け替えで同じキーのエッジが捨てられる場合と、根拠の移動を含む）と `set_status` を `apply_patch` で適用し、`load_graph(sid)` が `result.graph` と等しいことを確かめるストアのテストを足す。上の 2 つの変異で落ちることを PR 本文に書く。

### m1（Minor）: 1 回の取り込みで複数行を入れる場合の並びのテストが無い
- 根拠: `store/mod.rs:79` の `ORDER BY id` を `ORDER BY id DESC` にしても全テストが通る。session-store の往復テストは 1 回に 1 行ずつしか取り込まない。ADR §1「発話の並びは取り込み順（`row_id` の順）」。
- 修正案: 往復テストで 3 行をまとめて取り込み、`uid` と `row_id` が同じ順に並ぶことを確かめる。

### m2（Minor）: 抽出済みの発話を守る条件と、人の判断済みを守る条件のテストが無い
- 根拠: `store/mod.rs:173-174` の `AND extracted_rev IS NULL` を外しても、`store/mod.rs:208` の `AND review = 'auto'` を外しても全テストが通る。前者は同じ発話を再び入力に入れると `extracted_rev` が書き換わり、後者は承認済みのノードを却下に変えられる（ADR §5「人の判断は変えられない」）。
- 修正案: 抽出済みの発話を `input_uids` に入れても `extracted_rev` が変わらないテストと、判断済みのノード・エッジへの `set_review` が `UnreviewableTarget` になり rev も進まないテストを足す。

### m3（Minor）: 0 件のパッチの扱いが ADR の定義とずれ、落とした件数がどこにも残らない
- 根拠: 適用が 0 件のとき、入力の発話の `extracted_rev` に今の rev（初回は 0）を入れる（`store/mod.rs:171-177`）。ADR §1 の定義は「抽出に入れたパッチの rev」で、この値は入力と関係の無い前のパッチ（または存在しない rev 0）を指す。また、操作がすべて落ちた回は `graph_patches` の行が無いので、ADR §2「落とした数は `graph_patches.rejected_ops` に残す」が満たされない。Issue #14 は扱いを決められなければ Open questions に書くよう求めたが、PR 本文の Open questions は「なし」。安全面の問題は無い（rev は進まず、発話は二重に送られない）。
- 修正案: ADR 側で「0 件の回は `extracted_rev` に直前の rev を入れ、落とした件数はログにだけ出す」と追記してもらうよう Open questions に挙げる（実装は今のままでよい）。

### m4（Minor）: 未抽出の発話が再文字起こしの行かどうかを返さない
- 根拠: ADR §1「再文字起こしの行は … その回は参照用の直前の 3 発話を付けない」。`Utterance`（`model.rs:64`）にも `read_utterances` の SELECT にも `is_retranscribed` が無く、タスク 5 の consumer は判定できない。`transcript_segments` は既に JOIN しているので、列を 1 つ足すだけで返せる。
- 修正案: `pending_utterances` の戻り値で `is_retranscribed` を返す（`graph_utterances` の表は変えない）。タスク 5 に回すなら Issue に残す。

### n1（nit）: 話者ラベルの出力が `speaker = u32::MAX` で変わる
- 根拠: 前は `n + 1`（debug では panic、release では「話者0」）、今は `u64::from(n) + 1`（「話者4294967296」）。実際の話者番号では起きない。良くなる方向の変更なので、そのままでよい。PR 本文の「出力は今と同じ」に注記があるとよい。

### n2（nit）: 文字起こしの行が無いとグラフ全体が読めなくなる
- 根拠: `records.rs:23` は LEFT JOIN だが `Utterance.text` が `String` なので、行が無いと `InvalidColumnType` で `load_graph` も `apply_patch` も失敗し続ける（プローブで再現）。session-store は `transcript_segments` を消さないので、今は起きない。

## 判定
converged: no（Blocker 0 / Major 1 / Minor 4 / nit 2）
