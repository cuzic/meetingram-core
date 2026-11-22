# PR #29 レビュー（round 2）

対象: cuzic/meetingram-core `feat/r13-agenda-rules` @ 66cf691a4d44ede7c414e27df3f117f052f96e93
根拠: cuzic/meetingram `docs/adr/0001-discussion-graph.md`（r13、マージコミット e8dec585）。前ラウンド: `docs/reviews/pr29-round1.md`

## 結論（先に）
converged: **yes**（Blocker 0 / Major 0）。round 1 の M1 は直っている。増分と段 1 のプロンプトは、ADR §2 の 3 項目（案が対立する issue は必ず議題、細かな問いは案が対立していない事実・数値・可否の確認、それ以外は議題のまま）を ADR と同じ言い方で持つ。段 1 が外す対象は「細かな問いに当たらない issue の親と、進め方の問いへの親」になり、ADR §3 と一致する。Minor の指摘もすべて直っている。新しく見つけたのは nit 1 件だけ。

## 前ラウンドの解消状況
- M1: 解消。`extraction-r13.txt:1-3` が §2 の 3 項目になり、「議題の条件」という未定義の語は `src/` から消えた。`prompt.rs:39` の外す対象は ADR §3 の文と一致する。テスト（`tests/prompt.rs` の `extraction_and_issue_stage_distinguish_agendas_from_sub_questions`）は、両方のプロンプトについて 3 項目を行単位の完全一致で確かめ、段 1 の remove_edge の行も完全一致で確かめる。
- m1: 解消。`GraphStore::apply_patch_with_context` を追加し、`apply_patch` は `PatchContext::Other` で委ねる。変異（保存の入口で文脈を `Other` に固定する）で `store::tests::issue_consolidation_replaces_a_stored_parent` が落ちることを確かめた。このテストは、開き直した DB で親が付け替わり、`e1` とその根拠が消えていることまで見ている。
- m2: 解消。one-on-one の `i3` から `parent` を外し、README は「保留」にした。回帰テスト `one_on_one_read_through_remains_a_gold_agenda` がある。正解の親は m3 の `g4` だけになり、ADR §「計測」が名指しした例と一致する。
- m3: 解消。`evaluate` の doc（`eval.rs:72-76`）に、流し切りの直後で段 1 より前のグラフを渡すこと、段 1 と段 2 の操作を順に連結して渡すことが書かれた。
- m4: 解消。PR 本文の「公開 API」に、`GoldNode` の `parent` 追加（`#[non_exhaustive]` 化を含む）と `Evaluation` が `Copy` を失ったことを破壊的変更として書いた。`compile_fail` の doc test がある。アプリは `eval` を使っていない。
- n1: 解消。表に m1 `g5` を足し、m3 `g3` を「議題（最終の規則）」にした。

## 確認したこと

| 項目 | 結果 |
| --- | --- |
| `cargo fmt --all --check` | pass |
| `cargo clippy -p discussion-graph --all-targets -- -D warnings` | pass |
| `cargo test -p discussion-graph` | 77 passed、1 ignored、doc test 1 passed |
| `gh pr checks 29` | build 3 本と e2e-best-effort 2 本がすべて pass |
| 変異: 保存の入口で文脈を `Other` に固定 | `issue_consolidation_replaces_a_stored_parent` が失敗 |
| `tests/prompt.rs` の `limit` 4_000 → 4_200 | 増分のシステムプロンプトが長くなった分を上げただけ。上限の内に収まること、新しい発話 1 件、参照 3 件・200 文字のアサートはそのまま |
| 触ったファイル | `crates/discussion-graph/**` と PR 本文だけ |

## Findings

### n1（nit）: 保存の入口が文脈と `source` の組を検査しない
- 根拠: `crates/discussion-graph/src/store/mod.rs:167` の `apply_patch_with_context` は `PatchContext::IssueConsolidation` と `PatchSource::Live` / `Flush` の組も受け付ける。ADR は「段 1 の応答で、保存の `source` は `consolidate` のまま」としている。呼び出し側の誤りで、増分の応答に `remove_edge` を許してしまう。今の呼び出し側はテストだけ。
- 修正案（任意）: `IssueConsolidation` なのに `source != Consolidate` なら `StoreError::InvalidPatchSource` を返す。タスク 5 で入れてもよい。

## 判定
converged: yes（Blocker 0 / Major 0 / Minor 0 / nit 1）
