# PR #52 レビュー（round 2）

対象: meetingram-core fix/cloud-prompt @ 76e0f7178d64556899cfbcb4b2157bd527edfeea

## 結論（先に）

round 1 の M1・m1〜m4・n1 は、挙動とテスト（手書きの変異を含む）で解消を確かめた。spike r2 の判定から足した 4 つの規則
（dd を必ず 1 件、dd を定義から真にしない、言われていない制約は inferred で具体を作らない、a は到達したい状態）は、
ADR 0006 §1・§5 と矛盾しない（どれもプロンプトの規則で、検証の表・件数の上限・スキーマは変えていない）。
入力の `on` → `target` の改名は、プロンプト・描画・テストでそろっている。残るのは Minor 3 件と nit 4 件で、converged: yes。

## 前ラウンドの解消状況

- M1: 解消。料理の例（コンロ・煮込み・タイマー・かき混ぜる）は、spike の全会議（meetingram の `spikes/conflict-cloud/data/` の発話 jsonl と正解 JSON、
  `apps/e2e-scripted/scenarios/`）に出ない。`コンロ 煮込 タイマー かき混ぜ 鍋 調理 料理 台所 キッチン 火 家に` は 0 件。
  `混ぜ`（one-on-one「両方を混ぜると」）、`作れない`（m2「場を作れないか」）、`口`（「受け口」「窓口」）は別の意味の語で、題材は重ならない。
- m1: 解消。round 1 のプローブの形（a1 が p1 を支え p2 に反対し、a2 が a1 に反対する）を `a_reason_under_a_reason_listed_twice_does_not_repeat_its_relation` が見ている。
  重複を防ぐ仕組みは 2 つ（`targets` の重複除外と、`also` に積む前の同一判定）で、両方を外すとテストが落ち、片方だけでは落ちない（下の n4）。
- m2: 解消。1 段目の a2 の行に `also: [{objects_to, target: a1}]` が付き、`NoSecondLevelReasons` では外れる。
  「1 段目の行を探さない」「段で外さない」の変異はそれぞれ `a_first_level_reason_keeps_its_second_level_relation_until_that_level_is_dropped` で落ちる。
  `also` の `target` は 1 段目のノードの nid だけで、`allowed_nids`・`allowed_uids` に新しい値は入らない。
- m3: 解消。`cloud-system.txt:8` に優先の文がある（相手の案でも満たせる賛成の理由は a の手がかり、b・c は満たせない理由を元に stated、無ければ inferred）。ADR の文との差は m7。
- m4: 解消。round 1 で生き残った変異（injection の制限、a を具体的に、反論は bd に置く）と、新しい規則（dd を必ず 1 件、`also` の説明）の行を消すと、それぞれ `the_system_prompt_defines_what_each_assumption_may_say` が落ちる。
- n1: 解消。`cloud-system.txt:30` が `relation`・`target`・`also` を説明し、出力の `on` と別物だと書く。描画（`cloud.rs` の `"target"`）とテストの期待値も `target` にそろい、
  入力の行に `"on"` は残っていない（テストに残る `"on"` は出力の前提だけ）。コアのほかの場所は、この入力の形を読んでいない。

## Findings

### m5（Minor）: dd の「定義から真になる文」の例が、spike の m1/g4 の不合格文の言い換えになっている

- 根拠: `cloud-system.txt:20` の「ルールは 1 つなので両方にはできない」は、r2 の JUDGE.md「残った失敗 1」の m1/g4 run1・3
  「昼間の同じホットフィックスに適用する承認ルールは1つで…1人とも2人とも定めることはできない」の型で、m1 の会議には「今のルールだと、承認が2人必要」がある。
  悪い例なので M1 ほど強く引っ張らないが、測り直すなら m1/g4 の 3 回の dd を、その候補の失敗を避けるように動かす（round 1 の M1 と同じ種類）。
- 修正案: 料理の題材にそろえる（例:「献立は 1 つなので両方は作れない」）。

### m6（Minor）: このヘッドのプロンプトはまだ測っていない

- 根拠: spike r2 の依存は `fix/cloud-prompt#ed3581e`（meetingram の `spikes/conflict-cloud/Cargo.lock`）。r2 の (3) 93.5% は、
  round 1 の M1 の例（`design/i-notice` の答え）が入ったままのプロンプトで測っている。その後の変更（例の差し替え、dd を必ず 1 件、
  B・C を inferred にする条件、a は到達したい状態）は、効き方が測られていない。r2 で B・C の `inferred` が 11 → 38 に増えており、
  m3 の規則でさらに増える向きの変更なので、(2) と (4) の `stated` の件数が動きうる。基準からの余裕は大きい（i-notice の 6 回を除いても 80% を十分超える）ので、Major にはしない。
- 修正案: タグを打つ前に、このヘッドで sonnet の 22 候補 × 3 回（r3）を回すか、回さずにマージすることを owner が決めて PR か ADR に残す。PR 本文に、r2 で測ったコミットを書く。

### m7（Minor）: ADR 0006 §5 の書き方の規則が、プロンプトの B・C の規則に追いついていない

- 根拠: ADR §5 は「B・C は、案に付く賛成の理由があればそれを元にし、`stated` にする」と無条件に書くが、プロンプトは
  「相手の案でも満たせる賛成の理由は b・c にしない。無ければ inferred」とした（m3 への対応で、方向は正しい）。
  dd を必ず 1 件書くこと、a は到達したい状態であることも ADR に無い。どれも §1 の `basis` の規則と検証の表（同じ `on` に 2 件まで、全体で 8 件まで）とは矛盾しない。
- 修正案: meetingram の ADR 0006 §5 の「書き方の規則」に、この 3 点を追記する（アプリの repo の follow-up。spike の結果の追記と一緒でよい）。

### n2（nit）: 「dd は … 必ず 1 件書く」が「ちょうど 1 件」か「少なくとも 1 件」か読み分けられない

- 根拠: `cloud-system.txt:20`。同じ段の「同じ on に付ける前提は 2 件まで」と並ぶと、dd を 2 件書いてよいかが分からない。検証は 2 件まで通すので害は無い。
- 修正案: 意図どおり「dd は必ず 1 件以上書く（2 件まで）」か「dd はちょうど 1 件書く」にする。

### n3（nit）: 良い例が 1 つだけで、r2 で型をなぞると指摘された「1 つしかない／同じ」の言い回しのまま

- 根拠: r2 の JUDGE.md「新しく出た失敗 2」（dd 62 件中 49 件が「一律／1 つ／1 回／同じ」）は、例を 2 種類にするよう提案している。新しい良い例「家に 1 つしかないコンロの同じ口」も同じ型。
- 修正案: 型の違う良い例（手続き・人手など）をもう 1 つ、料理の題材で足す。

### n4（nit）: 重複を防ぐ 2 つの仕組みの片方ずつはテストで守られておらず、自己ループを飛ばす分岐にもテストが無い

- 根拠: `targets.retain(...)` だけ、または `(row.relation, row.on) != link && !row.also.contains(&link)` だけを無効にしても、テストは全部通る。`node.nid == target` を `false` にしても通る。
  今の入力では片方だけで足りるので、挙動は正しい。
- 修正案: どちらかを主にしてコメントに書くか、同じ (kind, target) のエッジが 2 本あるグラフのテストを足す。

### n5（nit）: PR 本文が今のヘッドと合っていない

- 根拠: 「変更」の段と Open questions が `also: [{relation, on}]`・`relation`/`on` のまま（今は `target`）。round 1 への対応の段は b・c の例を「タイマーを付ける、蓋をする」と書くが、実際は「かき混ぜる回数を決める」。
- 修正案: 本文を今のヘッドに合わせる。

## 確かめたこと（指摘なし）

- ADR §5 の矢印の 5 値と意味、PR #50 の ab/ac の文、JSON の例、D・D2 の扱い、116 文字の上限、「nids だけでは stated にならない」は変わっていない。
- 「入力に無い具体を作らない」「会議で言われていない制約は inferred」は、§1 の `stated` / `inferred` の区別を強める向きで、矛盾しない。
- `cargo fmt --all --check`: 通る。`cargo test -p discussion-graph`: 49 passed（ほか 18・31・20・1）。
  `cargo clippy -p discussion-graph -- -D warnings`: 警告なし。CI: build 3 件・e2e-best-effort 2 件とも pass。

## 判定

converged: yes（Blocker 0 / Major 0 / Minor 3 / nit 4）
