# PR #52 レビュー（round 1）

対象: meetingram-core fix/cloud-prompt @ ed3581e0cfc54b362e0886351d1f08eec94affae

## 結論（先に）

プロンプトの書き直しは JUDGE.md の修正案 1〜5 をほぼそのまま入れており、ADR 0006 §5 の書き方の規則・矢印の意味・JSON スキーマ・
PR #50 の ab/ac の向きの文とは矛盾しない。ただし dd の悪い例・良い例が、測り直しに使う 22 候補のうち `design/i-notice` と
`e2e-design/i-notice`（入力が同じ）の dd の不合格文と合格の答えをほぼ言い換えたものになっており、再計測の (3) を押し上げる（M1）。
`also` は入力の許可リストと 24,000 文字の段階を壊さないが、重複と取りこぼしが残る（m1・m2）。converged: no。

## 前ラウンドの解消状況

初回（Issue #51 の m4・n1 は下で確かめた）。

- #51 n1: 解消。`has_text` の `trim()` を外すと `assumption_rows_are_dropped_by_the_table_rules` が落ちることを確かめた（PR の自己申告どおり）。
- #51 m4: 2 つの 2 段目の argument に付く場合は解消（`also` を外す・関係を書き換える・`find` を無効にする変異はいずれも新テストで落ちる）。
  隣の 2 つの形が残る（m1・m2）。

## Findings

### M1（Major）: dd の例が、再計測に使う spike 候補の答えになっている

- 根拠: `cloud-system.txt:23-24` の悪い例「通知の出し方は 1 つに絞る必要があり、両方は採れない」と良い例
  「どちらの案も画面右上の同じ 1 つの欄に表示するので、同時には出せない」は、`design/i-notice`（問い「読み手の警告をどこで見せるか」、
  案「録音画面に短い警告を載せる」/「診断一覧にだけ警告を載せる」）と同じ題材の形をしている。
  JUDGE.md の `design/i-notice` sonnet の dd の不合格は「表示先は一つにする必要があり」「どちらか一方に決める必要」で悪い例と同じ型、
  `e2e-design/i-notice` run1 で合格した dd は「同じ欄の情報として扱うため」で、合成会議の u115「いま開いている表ではどちらも同じ欄になっている」に
  そのまま当たる。良い例はこの候補の正解を教えていることになる。2 候補 × 3 回 = sonnet 6 回分の dd が例に引っ張られ、
  基準との差（79.3% と 80%、前提 372 件で約 3 件分）より大きく動きうる。合成データなので個人データの問題ではないが、
  タスクの目的である「同じ 22 候補での測り直し」の判定を信用できなくする。
  b・c の行の手段の例（上限を付ける・一覧に出す・借りる）も `design/i2`・`i-notice`・`budget/i2` の不合格から取っている（こちらは語が抽象的なので影響は小さい）。
  なお JUDGE.md が提案した例（監視基盤）も `m3/g1` の不合格文の言い換えで、そのまま使っても同じ問題になる。
- 修正案: dd の例を spike の合成会議（`spikes/conflict-cloud/data/` の全会議）のどれとも題材が重ならないもの
  （例: 会議室の予約、倉庫の棚の割り当てなど）に替え、PR 本文に「例の題材は spike の候補と重ならない」ことを確かめた方法を書く。
  b・c の手段の例も候補に無い語にするか、例を外す。

### m1（Minor）: 両方の案に付く 1 段目の理由の下の 2 段目は、同じ関係が `also` に重複する

- 根拠: `cloud.rs:365` の 2 段目の `targets` は 1 段目の行から作るので、1 段目で 2 行になる argument（例: a1 が p1 を `supports` し p2 に `objects_to`）は
  2 回たどられる。2 回目は `seen` に当たり、同じエッジが `also` に積まれる。手元のプローブ（コミットしていない）で
  `rich_graph()` に `a1 objects_to p2` を足すと、a2 の行が `"relation":"objects_to","on":"a1","also":[{"on":"a1","relation":"objects_to"}]` になった。
  片方の案を支え、もう片方に反対する理由はよくある形なので、よく起きる。LLM には 2 本の関係があるように見える。
- 修正案: 2 段目の `targets` を重複なしにする（または `also` に積む前に `(kind, on)` が主の関係・既存の `also` と同じなら積まない）。重複しないことのテストを足す。

### m2（Minor）: 1 段目に出る argument が 1 段目の理由に付く関係は、まだ落ちる

- 根拠: `cloud.rs:368-377` は、`seen` に当たったノードを今の段（`level`）からしか探さないので、1 段目にいるノードの 2 段目の関係は捨てる
  （コメントも「A node already listed at the first level is not repeated」）。既存テスト `a_reason_on_both_levels_is_listed_once_at_the_first_level` の形
  （a2 が p2 を支え、a1 に反対する）で、a2 の行は `"on":"p2","relation":"supports"` だけになり、a1 への反対が LLM に渡らない（プローブで確認）。
  #51 m4 の「関係をその行に全部並べる」と同じ種類の取りこぼし。
- 修正案: 1 段目の行にも `also` を許し、2 段目で見つけた関係をその行に足す（1 段目のノードが 2 行あるときはどちらに足すかを決める）。
  `NoSecondLevelReasons` の段ではこの `also` も外すかどうかを決めて書く。

### m3（Minor）: B・C の「賛成の理由を元に stated」と「相手の案では満たせない要望」のどちらを優先するかが書かれていない

- 根拠: `cloud-system.txt:7` は「案に付く賛成の理由があれば、それを元にし … stated にする」（ADR 0006 §5 の文）、`:8-9` は「b は d2 では満たせない要望」
  「どちらでも満たせる要望は a に上げる」。d の賛成の理由が d2 でも満たせるとき（JUDGE.md の `m1/g1` の C がこの形）、どちらに従うかが決まらず、
  理由を a に上げて b を推測で作る（`stated` が減る）か、理由を元にした対立しない b を出すかで揺れうる。
- 修正案: 「賛成の理由のうち、相手の案では満たせないものを元にする。どれも相手の案で満たせるなら、それは a の手がかりにし、b は inferred で書く」のように優先を 1 文で書く。

### m4（Minor）: プロンプトのテストが新しい規則の一部しか見ていない

- 根拠: `cloud-system.txt` から injection の制限の行（「d・d2 そのもの、… 打ち手にしない」）、a を具体的にする行、
  「d2 が b を損なうという反論は bd に置く」を消しても、`cargo test -p discussion-graph` は全部通った（変異はコミットしていない）。
- 修正案: `the_system_prompt_defines_what_each_assumption_may_say` の needle に、a・injection・反論の置き場所の文を足す。

### n1（nit）: プロンプトが入力の `relation`・`on`・`also` を説明していない

- 根拠: system は arguments の行の欄に触れていない。`on` は出力の前提の `on`（矢印）と同じ名前で意味が違う（従来から）。`also` が増えて読み違いの余地が広がった。
- 修正案: 「arguments の relation・on は、その理由が on のノードを支える（supports）か反対する（objects_to）か。also はほかに付くノード」と 1 行足す。

## 確かめたこと（指摘なし）

- ADR §5: 書き方の規則（A は B・C の両方が仕える目的、B・C は要望、前提は偽でありうる理由、打ち手は前提を偽にする変更）、矢印の 5 値と意味、
  JSON の例、stated / inferred の規則、「nids だけでは stated にならない」、116 文字の上限はそのまま。PR #50 の ab/ac の文は変わっていない。
  D・D2 の扱い（slot、入れ替えない）も変わらない。
- `also` と §4: `on` は入力に載せた 1 段目のノードの nid だけで、`allowed_nids`・`allowed_uids` に新しい値は入らない（stated の抜け道は無い）。
  削る順（1 ノード 1 件 → 100 文字 → ほかの議題 → 2 段目）は変わらず、`NoSecondLevelReasons` では `also` ごと 2 段目が消える。
  `also` の文字数も system と user の合計で上限に数えられる。並びは `edges` の BTreeMap 順で決まり、実行ごとに変わらない。
- system の長さ: 1,293 → 2,350 文字。24,000 の約 4% を余計に使うが、上限の判定は system を含めて数えている。
- `cargo fmt --all --check`: 通る。`cargo test -p discussion-graph`: 47 passed（ほか 18・31・20・1）。
  `cargo clippy -p discussion-graph -- -D warnings`: 警告なし。CI: build 3 件・e2e-best-effort 2 件とも pass。

## 判定

converged: no（Blocker 0 / Major 1 / Minor 4 / nit 1）
