# PR #22 レビュー（round 2）

対象: cuzic/meetingram-core feat/consolidate-two-stage @ 670887a5da7834e59ddcadfe88bfa016ba3337ab（round 1 の d7198d3 からの差分を見た）

## 結論（先に）
converged: no。round 1 の M1・M2・m1・m2 は、挙動と変異で解消を確かめた。
ただし m1 の直し方（エッジを載せるのは、両端を含む**まとまり**があるときだけ）で、新しい Major が 1 件生じた。両端のノードが同じプロンプトに載っていても、別のまとまりにあるエッジは一覧から消える。
LLM がそれを出し直すと `EdgeDuplicate` で落ちる。これは S2 の 1 回目の失敗（エッジの一覧の欠け）と同じ形で、ADR r11 の「エッジは削らない」にも反する。
DoD の 4 コマンドは手元で成功し、`gh pr checks 22` は 5 件すべて pass。

## 前ラウンドの解消状況
- M1（根拠を外すと `allowed_uids` が空）: 解消。最後の段は `UID_ONLY = (0, 3)` になった。本文を付けず、各ノードの根拠の uid を最大 3 件載せ、`allowed_uids` にも入れる（段 1・段 2・Baseline のすべて）。
  `issue_stage_uid_only_evidence_still_allows_a_merge` があり、`(0, 0)` に戻す変異で 4 件のテストが落ちる。
- M2（Baseline の内容が変わった）: 解消。system は main の `CONSOLIDATE`（`extraction.txt`＋3 行）と同じ文字列で、`extraction.txt` も main と同一。r11 の規則は `extraction-r11.txt` に分けて増分だけに足している。
  順序はノード → エッジに戻った。`baseline_matches_original_prompt_when_it_fits` があり、system の差し替えとエッジを先頭に戻す変異のどちらでも落ちる。
- m1（ノードが複数のプロンプトに重複）: 解消。`groups()` が `assigned` を共有し、ノードは最初の issue のまとまりにだけ入る。たどる途中で既に割り当て済みのノードから先へ進まなくても、その先は前のまとまりで割り当て済みなので落ちない。
  変異（まとまりごとに独立にたどる）で `group_stage_assigns_shared_nodes_to_the_first_issue_only` が落ちる。
- m2（1 つのまとまりの超過で段 2 全体が失敗）: 部分的に解消。収まらないまとまりは `split_group` で二分され、他のまとまりは影響を受けない。分割を消す変異と、後半を捨てる変異のどちらでもテストが落ちる。
  ただし、UID だけでも収まらない 1 ノードは、依然として段 2 全体を `InputLimit` にする（doc コメントに明記。n1）。

## Findings
### M1（Major）: 同じプロンプトに両端があっても、まとまりをまたぐエッジが一覧から消える
- 根拠: `render` の `Layout::Groups`（`src/prompt.rs` の `include` の判定）は、`groups.iter().any(|group| group.contains(src) && group.contains(dst))` のときだけエッジを載せる。
  m1 の修正で共有ノードは 1 つのまとまりにしか入らなくなったので、共有ノードから 2 つ目の issue 側へのエッジは、2 つのまとまりを 1 回に詰めても必ず落ちる。
  例: 2 つの issue の position を両方 supports する argument、decision と issue の両方に付く action。
  プローブ: issue n1・n2、position n3→n1・n4→n2、argument n5 が n3・n4 を supports するグラフで、`build_consolidate_groups(.., 24_000)` は 1 プロンプトになる。n1〜n5 が全部載るのに、エッジの一覧は e1〜e3 だけで、e4（n5→n4）が無い。
  その e4 を `add_edge` で出し直すと `EdgeDuplicate` で落ちる。
  テスト `group_stage_keeps_unassigned_nodes_and_omits_cross_group_edges` も、1 プロンプトに収まる場合（`full`）でエッジ 2 件（a→p2 を欠く）を期待値にしている。
  ADR r11 §3 は「入力ではエッジの一覧をノードより先に置き…エッジとノードの本文は削らない」。仕様の m1 の「両端を含むまとまりが無ければ載せない」は、プロンプト（詰めた後の束）単位で読むべきところを、まとまり単位で実装している。
  二分の場合: 両半分に分かれた端点のエッジは、どちらのプロンプトにも載らない（プローブ: 上のグラフで上限を絞ると、5 つの 1 ノードのプロンプトになり、エッジは 0 件）。片方のノードが見えないので LLM が使うことは少ないが、出し直すと同じく落ちる。
- 修正案: エッジを載せる条件を、そのプロンプトに載るノード（`evidence` のキー、つまり束の和集合）が両端を含むこと、にする。
  さらに、片端だけが載るエッジも nid だけで載せる（round 1 の前の挙動）か、少なくとも両端が載るものは必ず載せる。`full` で 3 件を期待するテストに直す。
  どちらの範囲にするかは、仕様の文言を直すことと合わせて決める。

### m1（Minor）: 二分した断片は UID だけの根拠に固定され、他と詰められない
- 根拠: `split_group` は `[UID_ONLY]` の段だけで収まりを判定し、できたプロンプトを `split_prompts` に足して束とは別に返す。
  半分なら 200 文字の本文付きで収まる場合でも本文を外し（ADR の削る順序より強く削る）、小さな断片も他の束に詰めない。プローブでは、上限を絞ると 1 ノードずつの 5 回の呼び出しになった。
- 修正案: 断片にも `fit_group` の全段を当て、断片を通常のまとまりとして first-fit の詰め込みに戻す。

### n1（nit）: UID だけでも収まらない 1 ノードは、段 2 全体を失敗させる
- 根拠: `split_group` は `group.len() <= 1` で `Err(InputLimit)` を返し、`?` で段 2 全体が落ちる。ノードの本文は LLM の短い要約なので、system が上限に近い設定でしか起きない。doc コメントにも書かれている。
- 修正案: そのノードだけを飛ばして件数を返すか、今の挙動をテストで固定する。

## 判定
converged: no（Blocker 0 / Major 1 / Minor 1 / nit 1）
