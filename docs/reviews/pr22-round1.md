# PR #22 レビュー（round 1）

対象: cuzic/meetingram-core feat/consolidate-two-stage @ d7198d37ca2a934a3b02a750da5b19ba37a6e50f

## 結論（先に）
converged: no。
エッジを先に置いて切らないこと、段 1 が issue だけであること、段 2 の first-fit の詰め込み、削る順序、system に本文が入らないこと、`allowed_uids` の一致、増分の 3 規則は実装されていて、変異で落ちるテストもある。
ただし Major が 2 件ある。
M1: 根拠を「外す」段で `allowed_uids` が空になり、そのプロンプトへの応答の操作はすべて検証で落ちる。段 1 は 60 分の会議で現実にこの段に落ちる。
M2: 比較用の Baseline が、切り方以外（system とエッジの位置）まで変わっている。
DoD の 4 コマンドは手元で成功し、`gh pr checks 22` は 5 件すべて pass。

## 前ラウンドの解消状況
初回のため無し。

## 確かめたこと
- 変更範囲は `crates/discussion-graph/**` だけ。
- `render`（`src/prompt.rs:455-514`）は、どの経路・どの削り方の段でも `graph edges` を最初に全部出す。削るのは根拠（`evidence` の上限）だけ。
  根拠を外しても収まらなければ `InputLimit` を返す。エッジとノードの本文は削らない。
- 段 1: 生きている issue だけで、2 件未満なら `None`。根拠は先頭 1 件を 100 文字。上限を超えたら根拠を外し、それでも超えたら新しい issue から省き、`omitted_issues` に件数が入る。
- 段 2: issue に向かうエッジをたどったまとまりと、どの issue にも届かないまとまりを作る（ノードは落ちない）。first-fit で詰める。
  各まとまりの根拠は (全文, 全件) → (200, 全件) → (200, 1) → (0, 0) の順に削る。まとまりをまたぐエッジは、端点の片方を含むプロンプトに載る（`EdgeDuplicate` を避ける）。
- `allowed_uids` は `render` が載せた根拠の uid と同じ集合。上限の判定は system を含む（`Prompt::fits`）。
- system: `consolidation.txt`（＋`consolidation-edits.txt`、段ごとの定数）は固定の文字列で、new・reference の説明も発話・ノードの本文も含まない。増分の `extraction.txt` には ADR r11 §2 の 3 規則が入った。
- 公開 API: `build_consolidate_issues`・`build_consolidate_groups`・`Prompt::omitted_issues` の追加と、`ConsolidateStyle::Strengthened` の削除が PR 本文に書かれている。アプリ（meetingram main）は `discussion-graph` を v0.3.0 のタグで参照し、整理の関数はまだ呼んでいないので、今は壊れない。
- 変異（コミットしていない）を 10 個入れた。詰め込みを無効化（1 まとまり 1 回）、削る順序の入れ替え、まとまりをまたぐエッジを外す、つながらないまとまりを外す、段 1 に全種類を入れる、段 1 で古い方から省く、段 1 の 2 件未満の判定、上限から system を除く、Baseline の段を減らす、増分の action 規則を消す。
  どれも `tests/prompt.rs` のテストが落ちた。
- 添字 `self.nodes[nid]` は、生きたノードと生きたエッジの端点からしか来ないので panic しない。

## Findings
### M1（Major）: 根拠を「外す」段のプロンプトでは、どの操作も検証を通らない
- 根拠: 根拠を外した段（段 1 の `(0, 0)`、段 2・Baseline の `(0, 0)`）では、`allowed_uids` が空になる。
  §2 の規則の表は「`grounds` が空、`grounds` の `uid` がその回の入力に無い」操作を落とすので、この応答の `merge`・`add_edge`・`set_status` は必ず全部落ちる。テストもこの状態を期待値にしている（`tests/prompt.rs:194`・`:225`）。
  プローブ: issue 120 件（本文 40 文字、根拠 1 件）で `build_consolidate_issues(.., 24_000)` を呼ぶと、100 文字の根拠付きでは上限を超え、根拠なしの 12,248 文字に落ちて `allowed_uids` が空になる。
  そこで `merge n2→n1`（grounds u1）を `apply_patch` に通すと `GroundOutsideInput` で落ちる。
  S2 では増分の issue が正解の 4〜5 倍（15 分で 16〜37 件）だったので、タスク 4b の 60 分の会議の段 1 はこの経路に入りうる。その会議で issue の統合が 1 件も起きないのに、呼び出し側には成功に見える。
  ADR r11 の「外す」と §2 の根拠の規則が両立しないところで、仕様にない判断なので、実装で決めずに Open questions に上げるべきだった。
- 修正案: 外す段でも各ノードの根拠の `uid`（本文なし、または数十文字）を 1 件残し、`allowed_uids` に入れる。上限が許す間は、段 1 では根拠を外す前に古い issue を残して新しいものから省く、でもよい。
  どちらにするかを Open questions に書いて owner（ADR の改訂）に回す。少なくとも、`allowed_uids` が空のプロンプトは返さず、種類つきのエラーか印を返して、呼び出し側が「送っても無駄」と分かるようにする。

### M2（Major）: Baseline が切り方以外も変わっていて、S2 の対の比較の基準が動いている
- 根拠: 仕様（Issue #21 の 4）は「Baseline は内容はそのままにして、上限を超えたときにエッジを切らず根拠から削る形にだけ直す」。
  `build_consolidate`（`src/prompt.rs:233-254`）は、次の 2 点も変えている。
  - system を、以前の `extraction.txt`＋整理の 3 行（増分の new・reference の説明を含む）から、新しい整理専用の `consolidation.txt`＋`consolidation-edits.txt` に替えた。
  - エッジの一覧をノードの後から前に移した。
  ADR r11 は S2 の失敗の原因の 1 つに「増分用のシステムプロンプトの流用」を挙げ、2 段の整理の改善点にしている。Baseline にも同じ system を入れると、タスク 4b の対の比較は「強める前」との差を測れなくなる。
- 修正案: Baseline は以前の system 文字列（`extraction.txt`＋以前の 3 行）と、ノード → エッジの順をそのまま使い、上限を超えたときの削り方（根拠 (200,3) → (200,1) → 外す、エッジは切らない）だけを変える。
  外すと本文の後ろにエッジがあっても切れないので、順序を戻しても修正の目的は保たれる。新しい system を Baseline にも使いたいなら、その判断を Open questions に書く。

### m1（Minor）: 2 つのまとまりに属するノードが、別々のプロンプトに重複して載る
- 根拠: `groups()`（`src/prompt.rs:406-430`）は、issue ごとに独立にたどる。2 つの issue の position を支える argument や、decision と issue の両方に付く action は、2 つのまとまりに入る。
  詰め込みで別のプロンプトになると両方に載る。プローブ: issue 2 件にそれぞれ position、両方を supports する argument n5 を置き、上限を絞って 2 つに分けると、n5 が 2 つのプロンプトに載った。
  プロンプトは先に全部作るので、1 回目の `merge` で消えたノードへの 2 回目の操作は `MergeEndpoint` などで落ち、トークンも重複する。
- 修正案: 共有ノードは最初のまとまりだけに含める（他のまとまりにはエッジだけ載る）。または重複を許すことを doc に書き、テストで固定する。

### m2（Minor）: 1 つのまとまりが収まらないと、段 2 全体が `InputLimit` になる
- 根拠: `build_consolidate_groups` は `fit_group(..)?` で、1 つのまとまり（とくに、どの issue にも届かないノードの大きなまとまり）が根拠なしでも収まらないと、他のまとまりのプロンプトも含めて全部を捨てる。
  ADR §3 の「整理の失敗は 1 回で諦める」に沿うとしても、失敗の単位が段 2 全体になる。
- 修正案: 収まらないまとまりだけを飛ばして件数を返すか、つながらないノードのまとまりをさらに分ける。挙動を doc とテストで固定する。

## 判定
converged: no（Blocker 0 / Major 2 / Minor 2 / nit 0）
