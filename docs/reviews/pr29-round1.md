# PR #29 レビュー（round 1）

対象: cuzic/meetingram-core `feat/r13-agenda-rules` @ ff2aba5945702d5cb48eaa26db3b87660dc2d642
根拠: cuzic/meetingram `docs/adr/0001-discussion-graph.md`（r13、マージコミット e8dec585）。タスク仕様は Issue #28 と PR #29 の最新の「[codex] タスク仕様」コメント。
ただし ADR は、その仕様コメントの後（r13 のレビュー round 3 と 4）で文言が変わっている。照合は、マージされた ADR の文に対して行った。

## 結論（先に）
converged: **no**（Major 1）。`remove_edge`、段 1 の添え書き（(a)→(e) と記録される段階）、`position_count` と `has_objection`、§6 の細かな問いの数え方、評価（誤った `merge` の互いに素の判定、判定できない merge、正解の `parent` による分母からの除外）は ADR どおりで、主張された変異も落ちることを確かめた。
残る Major は 1 件で、段 1 のシステムプロンプトが、ADR の最終形の「§2 の細かな問いに当たらない issue の親を外す」ではなく、「議題の条件（position 2 件以上か反対あり）に当たる issue の親を外す」になっていること。さらに §2 の 3 項目め「それ以外は議題のまま残す」がどちらのプロンプトにも無い。このため、S2 の 2 回目で m1・m3 を潰した誤り（案が 1 件以下でも確認の問いではない issue が、別の議題の子にされる）を、段 1 が直すよう指示されない。

## 前ラウンドの解消状況
round 1 なので対象なし。

## 確認したこと

| 項目 | 結果 |
| --- | --- |
| `cargo fmt --all --check` | pass |
| `cargo clippy -p discussion-graph --all-targets -- -D warnings` | pass |
| `cargo test -p discussion-graph` | 74 passed、1 ignored（PR 本文と一致） |
| `gh pr checks 29` | build 3 本と e2e-best-effort 2 本がすべて pass |
| 変異: `remove_edge` の文脈の検査を外す | `remove_edge_requires_issue_consolidation_and_can_replace_a_parent` が失敗（主張どおり） |
| 変異: `matching` の正解の除外を外す | `gold_parents_exclude_sub_questions_and_their_private_nodes_from_recall` が失敗（主張どおり） |
| 変異: §6 で `status == Open` を外す／`!has(Resolves)` を外す | どちらも `only_open_sub_questions_without_an_answer_or_decision_are_unanswered` が失敗 |
| 変異: 誤った `merge` の `is_disjoint` を `!=` にする | `wrong_merges_require_two_disjoint_nonempty_gold_match_sets` が失敗 |
| 変異: `remove_edge` で `approved` のエッジを外せるようにする | `remove_edge_requires_...` が失敗 |
| 触ったファイル | `crates/discussion-graph/**` だけ（仕様どおり） |
| 正解の付け直し | 6 本の正解の全 issue について、`responds_to` する position の数と `objects_to` の有無を数えた。親を付けたのは m3 `g4`（position 0）と one-on-one `i3`（position 1、反対なし）の 2 件だけ。案が対立する issue に親を付けたものは無い。m3 `g4` は発話 23〜28・147 で「年間 700 万に収まるか」の数値の確認で、ADR §「計測」が名指しした例と一致する |

ADR との照合（問題なしのもの）:
- `remove_edge`: 段 1 の文脈でだけ許す、`sub_issue_of` だけ、生きたエッジが無ければ落とす、人の判断済みなら落とす、適用でエッジとその根拠を消し `AppliedOp` に残す、同じ応答の後続の `add_edge` で付け直せる。`apply_patch` は `PatchContext::Other` で従来どおり。
- 段 1: (a) 決定・案を `created_rev` の古い順に最大 2 件・各 60 文字 → (b) 決定 1 件・40 文字 → (c) 添え書きなし → (d) 根拠の本文を外す（`uid` は残す）→ (e) 末尾の issue を外す。`Prompt::issue_stage` に記録。`position_count` / `has_objection` は全段で載る。生きていないノード（`merged`・却下）は添え書きにも件数にも入らない。
- §6: 親のある issue は `open` で `responds_to` も `resolves` も無いときだけ未回答。議題は従来どおり `responds_to` が無いもの。
- 評価: 誤った `merge` は統合前のグラフで両端の候補集合を作り、両方が空でなく互いに素のときだけ数える。片方でも空なら `undetermined_merges` に `(from, into)` を応答順で入れる。正解の `parent` を持つ issue とその下にだけあるノード（とそこにつながる正解のエッジ）は再現率と辺の F1 の分母から外れる。

## Findings

### M1（Major）: 段 1 の「親を外す」条件と §2 の 3 項目めが ADR の最終形と違う
- 根拠:
  - ADR §3 段 1（e8dec585 の 359 行目）は、外す対象を「§2 の細かな問いに当たらない issue の親、進め方の問いへの親」としている。§2 の細かな問いは「事実・数値・可否の確認の問いで、案が対立していないもの」なので、案が 0〜1 件でも確認の問いではない issue の親も外す対象になる。
  - 実装の `ISSUE_OPERATIONS`（`crates/discussion-graph/src/prompt.rs:39`）は「議題の条件に当たる issue の親や、進め方の問いへの親は remove_edge で外してください」としている。外す対象が、position 2 件以上か反対ありの issue に狭まっている。これは改訂前の、position の数で分ける規則の名残り。
  - ADR §2 の 3 項目め（283 行目）「それ以外の issue（案が 1 件だけでも確認の問いでないもの）は議題のまま残す」に当たる文が、`crates/discussion-graph/src/extraction-r13.txt:1-3` にも `ISSUE_OPERATIONS` にも無い。さらに「議題の条件」という言葉は `extraction-r13.txt:2` と `prompt.rs:39` で使われているが、どこにも定義されていない（1 行目は条件を書いているが、その名前で呼んでいない）。
- 失敗の筋書き: 増分が m1 の `g3`（「A 社への説明をどうするか」、position 1 件、反対なし）を `g1`（リリース判断）の子にする。これは ADR §「計測」が S2 の 2 回目で m1・m3 について報告した誤りそのもの。段 1 のプロンプトでは `g3` は「議題の条件」に当たらないので、外せとは書かれていない。「細かな問い」の定義にも当たらないが、そうした issue をどう扱うかの文が無い。結果として親は残り、議題の数は正解より少ないままになり、r13 の狙いのうち「別の議題が親にされる」への対処が効かない。テスト `extraction_and_issue_stage_distinguish_agendas_from_sub_questions`（`tests/prompt.rs:695`）は部分文字列を見るだけなので、この抜けを見つけられない。
- 修正案: `extraction-r13.txt` に ADR §2 の 3 項目めを足し、1 行目の条件に「議題の条件」という名前を付ける（または語を使わずに書き下す）。`ISSUE_OPERATIONS` の外す対象を、ADR と同じ「細かな問い（事実・数値・可否の確認で、案が対立していないもの）に当たらない issue の親、進め方の問いへの親」に直す。テストで、両方のプロンプトに 3 項目め、段 1 に「細かな問いに当たらない issue の親」に当たる文があることを確かめる。

### m1（Minor）: 保存の経路から段 1 の文脈を渡せず、アプリでは `remove_edge` が必ず落ちる
- 根拠: `DiscussionGraphStore::apply_patch`（`crates/discussion-graph/src/store/mod.rs:160`）は `apply_patch` を呼ぶので、いつも `PatchContext::Other` になる。ADR の `remove_edge` 行は「呼び出し側が検証に段 1 の応答であることを渡す。保存の `source` は `consolidate` のまま」としている。この PR のタグをアプリ（タスク 5）が使うと、段 1 のプロンプトが `remove_edge` を出させても、すべて `RemoveEdgeContext` で落ちる。付け直しの `add_edge` も `IssueAlreadyHasParent` で落ちる。データは失われない（`rejected_ops` に数えられるだけ）。仕様は「保存はやらない」としているので、この PR の範囲では違反ではない。
- 修正案: タグを打つ前かタスク 5 の前に、`apply_patch` に文脈を渡せる保存の入口（例: `apply_patch_with_context` を通す引数）を足す follow-up の Issue を作る。`write_changes` は消えたエッジを DELETE し、根拠も差分で消すので、保存側の変更は入口だけで済む見込み。

### m2（Minor）: one-on-one の `i3` に親を付けた判断が ADR の定義から外れぎみ
- 根拠: `testdata/README.md:24`、`testdata/one-on-one.gold.json` の `i3`。発話 55「読み合わせは誰か頼めますか。初めて見た感じを一度聞いてから次へ進みたい」は、作業を頼む話（答えは 56「私なら十分ほど見られます」）で、親の `i2`「発表でどこまで説明するか」に答えるための事実・数値・可否の確認とは言いにくい。親を付けると、`p5` と行動 `t4`（「発表の読み合わせ枠を用意する」、根拠 123・125・140）が再現率の分母から外れる。1on1 から宿題が 1 件、評価の対象外になる。
- 修正案: 仕様の「迷うものは付けずに保留」に従って親を外し、表を「保留」にする。付けたままにするなら、表の理由に「`i2` の答えにどう効くか」を書く。

### m3（Minor）: `evaluate` の doc に、`before_consolidation` がどのグラフかが書かれていない
- 根拠: `crates/discussion-graph/src/eval.rs:60-68`。ADR タスク 4b (2) は「整理全体の前（流し切りの直後）のグラフ」と定めている。spike が段ごとに評価して、段 1 の後のグラフを渡すと、段 2 の `merge` の判定が ADR の定義からずれる。型では防げない。
- 修正案: doc に「流し切りの直後、段 1 より前のグラフを渡す。`consolidation_ops` は段 1 と段 2 の適用済みの操作を順に連結したもの」と書く。

### m4（Minor）: 公開 API の互換
- 根拠: `GoldNode`（`src/eval.rs:16`）は `#[non_exhaustive]` でないため、`parent` を足すと構造体リテラルで作る利用側は壊れる（このリポジトリのテストも `parent: None` を足している）。`Evaluation` は `Copy` を失った。どちらも PR 本文の「公開 API」に書かれているのは追加だけ。アプリは `eval` を使っていない（meetingram の grep で確認）ので、実害は無い。
- 修正案: PR 本文の「公開 API」に、この 2 つが破壊的変更であることを書く。タグは minor を上げる。

### n1（nit）: README の表から m1 `g5` が抜けている
- `testdata/README.md:16-25`。position 1 件の issue は表に挙げているのに、m1 `g5`（position 1 件、反対あり、親なし）だけが無い。m3 `g3` の「保留」は、ADR の最終形では「確認の問いではないので議題」と言い切れる（発話 76「実際どうするの」、決定 `g28` が付いている）。結果（親なし）は同じなので、理由の文だけ直せばよい。

## 判定
converged: no（Blocker 0 / Major 1 / Minor 4 / nit 1）
