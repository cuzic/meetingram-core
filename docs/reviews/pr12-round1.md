# PR #12 レビュー（round 1）

対象: cuzic/meetingram-core feat/discussion-graph-core @ 900be46941e3ce159ead54d3e2e3def82c0df752

## 結論（先に）
converged: no。§2 の規則の表は、各行と逐次の検証・連鎖・付け替え・人の判断の保護まで表どおりに実装されている。
DoD の 4 コマンドと CI は全緑で、testdata 6 本は出典と完全に一致した。
ただし、増分の文脈が「古いものから」ではなく「直近のパッチで触ったノードから」落ちる（ADR §2 違反、M1）。
また、評価の誤った `merge` が、強めた整理の規則どおりの正しい重複の統合を誤りと数える（タスク 4b の合否を歪める、M2）。

## 前ラウンドの解消状況
初回のため無し。

## 確かめたこと
- `cargo fmt --all --check`、clippy（workspace、Windows 向けの `capture-windows`）、`cargo test --workspace …` を手元で実行し、すべて成功した。`gh pr checks 12` は 5 件すべて pass。
- 変更範囲は `crates/discussion-graph/**`、ルートの `Cargo.toml`（members と default-members）、`Cargo.lock` だけ。
- testdata の 6 本を出典（`spike/discussion-graph-s1-measure` の meetings/gold、`spike/discussion-graph` の data/gold）と突き合わせた。発話の全項目、ノードの id・kind・text・grounds、エッジはすべて一致した。m3 は時刻が `null`。B の正解は出典に status が無いので、全ノードを `open` にしており README に記載がある。
- 規則の表: 「すべて」の行（grounds が空・入力外の uid・未知の act・confidence が範囲外・未知の op）、`add_node`（未知の kind・空の本文・tmp の重複・落ちた tmp への参照の連鎖）、`add_edge`（端点・種類の組・自己ループ・重複）、`merge`（同一・存在しない・`merged`・種類・人の判断済みの from・人の判断済みのエッジ・rejected の into、`merge A→B` の後の `merge A→C`）、`set_status`（存在しない・`merged`・`rejected`・人の判断済み・種類との組・`merged` の直接指定）。いずれも実装とテストがある。
- 付け替え: 自己ループと既存と同じ `(src, dst, kind)` は捨てられ、捨てたエッジの根拠も消える。`merged_into` は向け直され、連鎖が残らない。
- system には固定の文字列しか入らない。増分は `EXTRACT`、整理は `CONSOLIDATE`（＋`STRENGTHENED`）。
- 上限の計算には system も含まれる。参照用の発話は 3 件・200 文字、再文字起こしでは付かない。長い 1 件は本文を 1/3 に切り詰める。
- 整理の分割: issue ごとにまとまりを作り、各呼び出しに issue の一覧（nid と本文）を付ける。そのうえで 200 文字の切り詰め、根拠 1 件の順に縮める。
- Baseline は S1 のハーネス A の `build_consolidate`（根拠 3 件・200 文字・超過分を「以下省略」で切る）と同じ形。
- 信頼できない入力での panic: `Value` の添字（非オブジェクトは `Null` になる）、`prefix` の文字境界、`budget - context_len - ref_len` の下限、`fit_utterance` の二分探索、`render` の `self.nodes[nid]`（生きたノードとエッジだけから組む）を確かめた。panic の経路は無い。
- 変異（コミットしていない）: 付け替えの自己ループの除去、`merged_into` の向け直し、人の判断済みのエッジの検査、付け替えの重複の除去、捨てたエッジの根拠の削除、再文字起こしでの参照の省略、文脈の 1/3、分割時の issue の一覧、切り詰めの順序、上限に system を含めること、誤った merge の `is_some`、`retracted` の種類の検査、rejected の into の検査を外した。どれも既存のテストが落ちた。生き残ったのは `node()` の `seen_tmp` の検査を消した変異だけ（m2）。

## Findings
### M1（Major）: 増分の文脈が、古いものではなく直近のパッチで触ったノードから落ちる
- 根拠: `src/prompt.rs:121-155` は、文脈を「open の issue（新しい順）→ 直近のパッチで触ったノード（新しいパッチ順）」の順に並べる。`:64-68`・`:90-93` はそれを末尾から `pop` する。そのため、直近のパッチで触ったノードが、何回も前の open な issue より先に必ず落ちる。
  ADR §2 は「文脈全体で入力の上限の 1/3 の文字数まで（超えたら古いものから落とす）」。
  プローブ: n1〜n5 を open の issue（created_rev 1〜5）、n6 を直近のパッチで触った position（created_rev 6、最も新しい）にし、各本文を 480 文字にした。limit 3,500 で `build_incremental` を呼ぶと、文脈は `n5, n4` になり、n6 が落ちる。
  既存のテスト `incremental_limits_context_and_drops_oldest_and_hidden_nodes`（`tests/prompt.rs:26-60`）もこの順を固定している。最も新しい n6 が落ち、n4・n5 が残ることを期待値にしている。
  影響: 長い会議で文脈が 1/3 を超えると、直前に話していた position や argument が文脈から消える。重複のノードが増えやすくなる（S1 の適合率の主因）。
- 修正案: 落とす順を新しさで決める。open の issue と触ったノードを合わせ、`updated_rev`（同じなら `created_rev`・nid）の古い順に落とす。テストは「新しいものが残り、古いものから落ちる」を期待値に直す。
  open の issue を常に優先したい意図があるなら、ADR に無い判断なので Open questions に書く。

### M2（Major）: 誤った `merge` の数が、正しい重複の統合を誤りと数える
- 根拠: `src/eval.rs:71-82` は、整理の前のグラフにも 1 対 1 の対応付け（`matching`）を使う。整理の前には重複があるので、同じ正解ノードに当たる予測ノードのうち 1 つしか対応しない。対応しなかった重複を `into` にした `merge` は、「正解に対応する from を正解に対応しない into に入れた組」として誤りに数えられる。
  プローブ: 正解 g1（根拠は行 1）に対し、整理の前に n1（u1）と重複の n2（u1、u2、u3）を置き、`merge n1→n2` を適用した。強めた規則「根拠が多い方を into に」どおりの正しい統合である。それでも `evaluate` は `wrong_merges = 1` を返す（整理の後の recall は 1.0）。
  タスク 4b の合否は「誤った merge が 6 本の合計で 2 件以下」なので、強めた整理ほど誤りが水増しされ、正しい統合が不合格の原因になりうる。
- 修正案: 誤った merge の判定では、排他的でない対応を使う。整理の前の各ノードを、同じ種類で根拠の Jaccard が最大の正解ノードに写す（重なりが無ければ対応なし）。そのうえで ADR の 2 条件を当てる。
  この写し方の手書きのテスト（重複の小さい方を大きい方へ統合しても 0 件）を足す。ADR の「対応する」を 1 対 1 と読むべきだと考えるなら、実装で決めずに Open questions に書く。

### m1（Minor）: 既存の nid と同じ名前の `tmp` が、既存のノードを隠す
- 根拠: `src/patch.rs:263-271` の `node()` は `tmp` を先に引く。`{"op":"add_node","tmp":"n1",…}` の後では、同じ応答の `n1` は既存の n1 ではなく新しいノードを指す。
  プローブ: 既存の issue n1 があり、応答が `add_node tmp=n1（position）`、`add_node tmp=x（position）`、`add_edge x→n1 responds_to` のとき、最後のエッジは既存の n1 への回答のつもりでも `EdgeKind` で落ちる。
  `tmp` が落ちた場合も、`seen_tmp` が既存の n1 への参照をすべて落とす。
- 修正案: 既存の nid と同じ `tmp` を持つ `add_node` は落とす（`DuplicateTmp` か新しい理由で数える）。テストを足す。

### m2（Minor）: 落ちた `tmp` の連鎖のテストが、`seen_tmp` の検査を効かせていない
- 根拠: `node()` の `if self.seen_tmp.contains(id) { return None; }` を消しても、全テストが通る。`rejects_duplicate_tmp_and_references_to_dropped_tmp` の落ちた tmp `b` は既存の nid と重ならないので、検査が無くても `graph.nodes.get("b")` が `None` を返す。
- 修正案: m1 を直すときに、既存の nid と同じ名前の落ちた tmp を指す後続が落ちる（または m1 の規則で add_node 自体が落ちる）テストを足す。

### m3（Minor）: 何も適用しなくても `rev` が進む
- 根拠: `src/patch.rs:126` は、操作が 0 件でもすべて落ちても `graph.rev += 1` する（プローブ: 空の ops で rev 0 → 1）。
  ADR §2 は「適用した操作は 1 つのパッチ（rev を 1 つ進める）として…保存する」。タスク 3 でそのまま保存すると、空のパッチが `graph.{sid}.updated` と UI の読み直しを引き起こしうる。
- 修正案: `applied` が空なら rev を進めない。または「空でも rev を消費する」ことを doc コメントに書き、タスク 3 の扱いを PR 本文に書く。

### n1（nit）: 付け替えで重複したエッジのどちらを残すかが eid の文字列順に依存する
- 根拠: `redirect_edges`（`src/patch.rs:407-440`）で、動かすエッジ同士が重複したときに残るのは `BTreeMap` の順で先のものになる。`"e10" < "e2"` なので、残る根拠が eid の桁数で変わる。挙動の正しさには影響しない。
- 修正案: eid の数値の順で並べるか、捨てるエッジの根拠を残す側に移す。

### n2（nit）: `Op` が型の無い `serde_json::Value`
- 根拠: 仕様（Issue #10 の 3）は「`Op`（add_node / add_edge / merge / set_status、ADR §2 の JSON の形）」。`Op(pub Value)` は形を持たない。不正な 1 件で全体が落ちないようにする意図は理解できるし、`AppliedOp` が型を持つので実害は小さい。
- 修正案: doc コメントに意図を書くか、`parse_ops` の後に 1 件ずつ型へ写す。

## 判定
converged: no（Blocker 0 / Major 2 / Minor 3 / nit 2）
