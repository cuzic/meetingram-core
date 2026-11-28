# PR #50 レビュー（round 1）

対象: cuzic/meetingram-core feat/conflict-cloud-core @ a8a78ccf676fd516dd82e74c3c7cf9228cad15a5

## 結論（先に）
converged: no。保存（`graph_analyses`、置き換え、`delete_session`、`BEGIN IMMEDIATE`）、候補の条件と並び順、入力の削る順と「大きすぎる」、
検証の表（`nids` だけの `stated` の格下げ、UTF-16 の 116 単位、前提の件数、全体の失敗、D・D2 の上書き）は ADR 0006 に合っており、テストも効いている。
ただし system プロンプトが矢印 `ab`・`ac` の向きを逆に説明しており（M1）、issue に直接付く宿題が入力に入らない（M2）。どちらも ADR 0006 §4・§5 の違反。

## 前ラウンドの解消状況
round 1 のため無し。

## 確かめたこと
- CI: build 3 ジョブとも pass（e2e-best-effort は pending。best effort）。
- 手元: `cargo fmt --all --check` 通る、`cargo clippy -p discussion-graph --all-targets -- -D warnings` 通る、
  `cargo test -p discussion-graph` 通る（lib 41 件）。
- 手書きの変異（コミットしていない）。落ちたもの: 並び順から `has_objection` を外す、`decided` を常に false、同じ `on` の上限を 3、
  全体の上限の比較を `>`、ほかの議題で親の除外を外す、ほかの議題を新しい順、根拠の発話を新しい順、1 ノード 1 件を無効、
  100 文字の段を 200、段 3 と段 4 の入れ替え、D の `basis` を `inferred`、LLM の `d` を切らない、未知の `basis` を `stated`、
  空の本文の検査を外す、置き換えで古い `body_json` を残す。生き残ったもの: m3。
- ログ: `cloud.rs`・`store/analyses.rs` に `tracing` の呼び出しは無い。エラー型は固定文。DB の処理に `unwrap`・`expect`・添字は無い
  （`cloud.rs` の添字は、直前の存在確認か `len() >= 2` の後で、ロックの外の純粋関数）。
- 既存の `apply_patch` とプロンプト: `prompt.rs` は `chars`・`prefix` の可視性だけの変更で、既存のテスト（lib・統合）はそのまま通る。
- 「実装で決めたこと」の 2 点の判断:
  - `dropped` の数え方（要素・前提・打ち手に加え、落とした `grounds`・`nids` を 1 件ずつ数える）: ADR 0006 §5 の表は根拠についても
    「その根拠だけ落とす」と書いており、それを数えるのは表の読み方として妥当。受け入れる。ただし m1 の数えすぎは直す。
  - D・D2 の `grounds` に入力に載せなかった発話を含めること: §5 の適用順の注「『各要素』と『根拠』の行は D・D2 には当てない」と、
    §1 の「写したノードの根拠の発話を `grounds` に持つ `stated` とする」に合う。position の根拠はグラフの実在の発話で、画面から開ける。受け入れる。

## Findings

### M1（Major）: system プロンプトが矢印 `ab`・`ac` の向きを逆に説明している
- 根拠: `crates/discussion-graph/src/cloud-system.txt:9` は「ab は『b を満たすには a が要る』、ac は『c を満たすには a が要る』」。
  対立解消図では A が B と C の仕える目的で（ADR 0006 §5 の書き方の規則「A は B と C の両方が仕える目的」、同じファイルの 5 行目も同じ）、
  矢印 `ab` は「A を満たすには B が要る」。`bd`・`cd` の説明（「d は b に要る」）とも向きがそろっていない。
  モデルは `ab`・`ac` の前提を逆向きの関係として書くことになり、タスク 2 の spike の判定 (3)（前提が矢印の裏の信念か）を、プロンプトの誤りで下げる。
  固定文なので単体テストでは捕まらない。
- 修正案: 「ab は『a を満たすには b が要る』、ac は『a を満たすには c が要る』、bd は『b を満たすには d が要る』、cd は『c を満たすには d2 が要る』」と、
  5 つの矢印を同じ言い方にそろえる。

### M2（Major）: 題材の issue に直接 `assigns` で付く宿題が入力に入らない
- 根拠: ADR 0006 §4 の入力は「題材の issue に付く decision と action の本文（決定済みのとき）」で、ADR 0001 §1 の `assigns` は
  「action → decision か issue」。`crates/discussion-graph/src/cloud.rs:401-410` は、集めた decision に `assigns` する action だけを拾い、
  `edge.dst == issue.nid` の action を拾わない。プローブ（`rich_graph()` に `act2 --assigns--> i1` を足して `build_conflict_cloud_default`）で、
  `act2` の本文が user に無いことを確かめた。この経路のテストも無い。
- 修正案: 決定済みのとき、`assigns` の宛先が題材の issue か集めた decision である生きた action を拾い、issue に直接付く宿題のテストを足す。

### m1（Minor）: 件数の上限で落とした前提の格下げと根拠を、結果に残らないのに数えている
- 根拠: `cloud.rs:705` で `checker.element` が根拠の検査と格下げを数えてから、711 行で件数の上限を見る。
  プローブ: `on: "ab"`、`basis: "stated"`、`grounds: ["u99"]` の前提を 3 件 → 残るのは 2 件だが `downgraded = 3`、`dropped = 4`
  （根拠 3 件 + 前提 1 件）。保存される `downgraded` が図に無い要素を数える。
- 修正案: 本文の空の検査 → 件数の上限 → 根拠と格下げの順にし、上限で落とした前提は「前提 1 件」だけを数える。テストを足す。

### m2（Minor）: system プロンプトに「B・C は賛成の理由を元にしたとき `stated` にする」が無い
- 根拠: ADR 0006 §5 の書き方の規則は「B・C は、案に付く賛成の理由があればそれを元にし、`stated` にする」。
  `cloud-system.txt:6` は「それを元にする」までで、`stated` にすることと、その理由の根拠の `uids` を `grounds` に入れることを書いていない。
- 修正案: M1 と同じ修正で、b・c の行に「元にした理由の uids を grounds に入れ、stated にする」を足す。

### m3（Minor）: 2 段目の argument の重複除外がテストで守られていない
- 根拠: `cloud.rs:366` の `if second_level && seen.contains(...)` を `if false` にしても、lib の 41 件がすべて通る。
  外すと、1 段目にもある argument が 2 段目として重複して入り、段 4（2 段目を外す）で 1 段目の行まで消えるかが見えなくなる。
- 修正案: position と別の argument の両方に付く argument を置き、2 段目として重複しないこと、段 4 で残ることのテストを足す。

### n1（nit）: PR 本文の「本文は Debug に出さない」は、エラー型だけに当てはまる
- 根拠: `CloudPrompt`・`CloudAnchor`・`CloudDiagram` は `Debug` を derive しており、`{:?}` で本文が出る。既存の `Prompt` も同じなので挙動の問題ではない。
- 修正案: PR 本文の書き方を「エラー型の Display・Debug に本文を持たせない」に直す（コードの変更は不要）。

## 判定
converged: no（Blocker 0 / Major 2 / Minor 3 / nit 1）
