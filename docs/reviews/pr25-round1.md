# PR #25 レビュー（round 1）

対象: cuzic/meetingram-core feat/sub-issue-of @ 64ec32b0c2eaa44a64149ad9e9ad09b3b1226eb8

## 結論（先に）
converged: yes。
`sub_issue_of` の検証（親は 1 つ、循環しない）は、同じ応答の tmp の連鎖でも、`merge` の付け替えでも効く。
評価の除外（細かな問いと、その下に**だけ**あるノード）は、議題側にも付くノードを外していない。
段 1・段 2 のプロンプトは PR #22 の保証（エッジを落とさない、`allowed_uids`、上限）を保っている。
Blocker・Major は無く、Minor 2 件（アウトラインから議題の外のノードが黙って消える、テストの穴）と nit 1 件。
DoD の 4 コマンドは手元で成功し、`gh pr checks 25` は 5 件すべて pass。

## 前ラウンドの解消状況
初回のため無し。

## 確かめたこと
- 変更範囲は `crates/discussion-graph/**` だけ。
- 検証（`validate_issue_parent`）: 親の判定は、生きているエッジ（`live_edges`）の `sub_issue_of` から作る。却下されたエッジや、`merged`・`rejected` の親は数えない。
  循環は `dst` から親をたどり、`visited` で打ち切る。`add_edge` は、それまでに適用した同じ応答のエッジを含む状態で検査する。
  テスト（a→b、b→c、a→d、c→a）で、親の 2 つ目と、tmp の連鎖による循環が落ちる。
- `merge` の付け替え: `from` は先に `merged` になり、`from` のエッジは生きているエッジから外れてから 1 本ずつ戻される。そのため、付け替えた `sub_issue_of` は次のように検査される。
  - `into` に既に親があれば落ちる（`from` の親は引き継がない）。
  - `from` の子は `into` の子になり、`into` の祖先に戻るなら落ちる。
  - `from`・`into` が親子なら自己ループとして落ちる。
  手で追った例（X→Y、Z→X で Y を Z に統合すると X→Z は循環）も落ちる。重複のキーで置き換える経路は、同じ親子なので検査が要らない。
- 評価: `excluded = (細かな問いから responds_to などを逆にたどったノード) − (議題から同じくたどったノード)`。
  プローブ: 議題 n1、細かな問い n2（→n1）、n2 にだけ答える position n3、n1 に答える n4、n3 と n4 を両方 supports する n5、n2 を resolves する n6。
  除外は n2・n3・n6 で、n5（議題側にも付く）は除外されず、precision・recall とも 1.0。どの issue にも付かないノードは除外されず、適合率の分母に残る。
  エッジの F1 も除外したノードに触れるエッジを数えない。誤った `merge` は仕様どおり変えていない。
- プロンプト: 増分の文脈の issue に `parent_nid` が付き、r12 の規則は `extraction-r12.txt` で増分だけに入る（Baseline の system は変わらない）。
  段 1 の各 issue に `parent_nid`（無ければ `null`）が付き、両端が載る `sub_issue_of` のエッジが載る。根拠の uid だけの段でも `allowed_uids` が空でない（テストあり）。
  段 2 は親の無い issue を根にし、`sub_issue_of` もたどるので、子の issue とその下が議題のまとまりに入る。エッジの一覧の規則（両端なら通常、片端なら external）、上限の判定、uid だけの段は PR #22 のまま。
- 防御: `unanswered_by_agenda` と `build_outline` は循環を `visited` / `path` で打ち切る。`apply_patch` では作れない循環を手で入れたグラフでも、panic も無限ループも無い（プローブ）。
  `build_outline` の `graph.nodes[&edge.src]` は生きているエッジの端点なので panic しない。
- 公開 API: `EdgeKind::SubIssueOf`、`Rejection::{IssueAlreadyHasParent, IssueParentCycle}`、`Evaluation::{agendas, sub_questions}`、`unanswered_by_agenda`、`build_outline`、`OutlineNode` が PR 本文に書かれている。
  `Evaluation` は `#[non_exhaustive]` でないので、フィールドの追加は形式上は破壊的変更。ただし crate の外で組み立てる場所は無い。
- 変異（コミットしていない）を 16 個入れた。循環の検査、親 1 つの検査、`add_edge` での検査、付け替えでの検査、評価の除外（外さない・全部外す・分母・エッジ）、段 1 の `parent_nid`、増分の `parent_nid` と規則、議題ごとの集計の根のたどりを外すと、どれもテストが落ちた。
  生き残ったのは m2 と n1 の 4 つ。

## Findings
### m1（Minor）: アウトラインの木から、議題の下に無いノードが黙って消える
- 根拠: `build_outline`（`src/model.rs`）は、親の無い生きている issue を根にして、エッジを逆にたどった先だけを返す。
  どの issue にもつながらない position・argument・decision・action（`responds_to` が作られなかった、または答え先の issue が却下された）は、木のどこにも出ない。
  プローブ: 議題 n1、孤立した position n5、手で入れた循環 n2⇄n3 とその下の n4。`build_outline` は n1 の 1 ノードだけを返し、n2〜n5 が消える。循環は検証で作れないが、孤立したノードは普通に生じる。
  ADR §5 のアウトラインはタスク 6 がこの関数で描くので、画面から黙ってノードが消える。
- 修正案: 根に届かない生きているノードを、別の一覧（`Outline { agendas, unattached }` など）で返すか、doc コメントに明記してタスク 6 の仕様に書く。テストを足す。

### m2（Minor）: 3 つの新しい挙動にテストが無い
- 根拠: 次の変異は、すべてのテストを通った。
  - `valid_edge` の `SubIssueOf => (Issue, Issue)` を `true` にする。issue 以外の端点の `sub_issue_of` が落ちることを確かめていない。
  - 段 2 の根から「親を持つ issue」を除く filter を消す。子の issue が議題のまとまりに入ることは、nid の順にたまたま依存して確かめていない（仕様のスコープ 5）。
  - 段 1 で `sub_issue_of` のエッジを載せる条件を外す。
  挙動はどれも正しい。
- 修正案: position → issue の `sub_issue_of` が `EdgeKind` で落ちるテストを足す。
  子の issue の nid が親より小さいグラフで、段 2 の `graph groups` が 1 つのまとまりになるテストを足す。段 1 の `graph edges` に `sub_issue_of` が載るテストを足す。

### n1（nit）: アウトラインの子の順序（細かな問いを先に）が固定されていない
- 根拠: `build_outline` の `sort_by_key` を消しても、テストが通る。表示の順序だけの問題。
- 修正案: 子の issue と position が混ざる木で、順序を確かめる 1 行を足す。

## 判定
converged: yes（Blocker 0 / Major 0 / Minor 2 / nit 1）
