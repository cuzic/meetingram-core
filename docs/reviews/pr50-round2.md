# PR #50 レビュー（round 2）

対象: cuzic/meetingram-core feat/conflict-cloud-core @ 65c44b9a3ded9e5a1dd59169716b514f1bce9951

## 結論（先に）
converged: yes。round 1 の M1・M2・m1〜m3 は差分とテストで解消し、新しいテストはそれぞれ手書きの変異で落ちた。
新しく Minor 1 件（2 段目の重複除外で、2 本目の関係が入力から消える）と nit 1 件があるが、Blocker・Major は無い。follow-up で扱ってよい。

## 前ラウンドの解消状況
- M1: 解消。`cloud-system.txt:9` は ab「a を満たすには b が要る」、ac「a を満たすには c が要る」、bd「b を満たすには d が要る」、cd「c を満たすには d2 が要る」で、5 つの矢印の言い方がそろった。`the_system_prompt_states_the_arrows_and_the_stated_rule` で固定した。
- M2: 解消。`cloud.rs:403-410` は、生きた decision があるとき、宛先が題材の issue か集めた decision の action を拾う。`an_action_assigned_directly_to_the_issue_is_in_the_input_only_when_decided` は直接付く宿題が入ること（nid・根拠の uid も）と、決定が無ければ入らないことを確かめる。変異「直接の宛先を外す」「決定済みの条件を外す」のどちらでも落ちた。
- m1: 解消。前提は `on` → 本文（`has_text`）→ 件数の上限 → 根拠と格下げ（`element`）の順になった。上限で落ちた前提は `dropped` に 1 だけ足す。`assumptions_dropped_by_the_quota_add_one_to_dropped_and_nothing_else` で `downgraded = 2`、`dropped = 5` を確かめる。round 1 の順に戻す変異で落ちた。結果（残る前提）は ADR 0006 §5 の「`on` の検査と本文の検査で落とした後に、出てきた順で数える」と変わらない。
- m2: 解消。b・c の行に「その理由の根拠の uid を grounds に入れて stated にする」が入った。
- m3: 解消。`a_reason_on_both_levels_is_listed_once_at_the_first_level` が、1 段目にある argument を 2 段目に繰り返さないことと、段 4 で 1 段目の行が残ることを確かめる。`seen` の検査を外す変異と、round 1 の `contains` に戻す変異の両方で落ちた。

## 確かめたこと
- CI: build 3 ジョブと e2e-best-effort 2 ジョブがすべて pass。
- 手元: `cargo fmt --all --check` 通る、`cargo clippy -p discussion-graph --all-targets -- -D warnings` 通る、`cargo test -p discussion-graph` 通る（lib 45 件）。
- 差分は `crates/discussion-graph/` の 3 ファイルだけ（`cloud-system.txt`、`cloud.rs`、`cloud/tests.rs`）。保存と既存のプロンプトは変わっていない。
- `element` が `None` を返す枝（`cloud.rs:727-730` 付近）は、`has_text` と `element` が同じ本文の検査（trim して空でない）をするので、今は通らない。

## Findings

### m4（Minor）: 2 つの argument に付く 2 段目の argument は、2 本目の関係が入力から消える
- 根拠: `cloud.rs:366` の `!seen.insert(...)` で、2 段目の argument は最初に見つけたエッジの 1 行だけになる。新しいテストは `a4`（`a1` を supports、`a2` を objects_to）を `("a4", "a1", true)` の 1 行だけと固定しており、「`a4` は `a2` に反対する」が LLM に渡らない。1 段目の argument が別の 1 段目の argument にも付く場合（テストの `a2 → a1` の objects_to）も同じく関係が消える。一方、1 段目で両方の position に付く argument は 2 行になり、扱いがそろっていない。ADR 0006 §4 の入力は argument の「本文・種類・nid」で、種類（関係）が一部欠ける。ノードの本文と根拠は残るので Minor。
- 修正案: ノードは 1 回だけ出し（段 4 で外すかどうかは今のとおり最初の段で決める）、関係は `relations: [{"relation", "on"}, …]` のようにその行に全部並べる。

### n1（nit）: 空白だけの本文の前提のテストが無い
- 根拠: `has_text` の `trim()` を外す変異（空白だけの前提が件数の枠を使う）で、lib の 45 件がすべて通った。今の挙動は正しい。
- 修正案: `assumption_rows_are_dropped_by_the_table_rules` の `item("ab", "")` に `item("ab", "  ")` を足す。

## 判定
converged: yes（Blocker 0 / Major 0 / Minor 1 / nit 1）
