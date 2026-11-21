# PR #22 レビュー（round 3）

対象: cuzic/meetingram-core feat/consolidate-two-stage @ a5e2132e89f16cb3266283ba2300f95765ce26b5（round 2 の 670887a からの差分を見た）

## 結論（先に）
converged: yes。round 2 の M1・m1・n1 は、round 2 のプローブの再実行と変異で解消を確かめた。差分に新しい Blocker / Major は無い。
DoD の 4 コマンドは手元で成功し、`gh pr checks 22` は 5 件すべて pass。
公開 API の変更（`build_consolidate_groups` が `(Vec<Prompt>, usize)` を返す）は PR 本文に書かれている。

## 前ラウンドの解消状況
- M1（まとまりをまたぐエッジが一覧から消える）: 解消。段 2 では、そのプロンプトに両端が載るエッジは通常の行で、片端だけのエッジは `"external": true` 付きで載る。
  system に「external のエッジも既に存在するので出し直さない」が足された。
  round 2 のプローブ（issue n1・n2、position n3・n4、両方を supports する argument n5）で、24,000 文字の 1 プロンプトに e1〜e4 が全部載り、`external` は付かない。
  上限を system＋0〜1,600 文字まで 10 文字刻みで変えて調べた。どの上限でも次の 3 つが成り立った。
  - 全プロンプトに載ったノード＋省いた数がちょうど 5（落ちも重複も無い）。
  - 省いたノードが無ければ e1〜e4 がどれかのプロンプトに載る。
  - 各プロンプトが上限内で、`allowed_uids` が空のプロンプトは無い。
  round 2 で出た「エッジ 0 件の 1 ノードのプロンプト 5 つ」は無くなり、1 ノードのプロンプトにも external のエッジが付く。
  変異: external の行を消す、両端が同じまとまりにあるときだけ載せる（round 2 の実装）、system の注記を消す。どれもテストが落ちる。
- m1（二分した断片が UID だけに固定され、詰められない）: 解消。断片には `GROUP_EVIDENCE_TIERS[1..]`（200 文字 → 1 件 → UID だけ）を当て、ほかのまとまりと同じ first-fit の詰め込みに戻す。
  プローブでも上限を広げるにつれて 5 → 4 → 3 → 2 → 1 プロンプトにまとまった。変異（断片を UID だけにする、詰め込みを無効にする）でテストが落ちる。
- n1（収まらない 1 ノードで段 2 全体が失敗）: 解消。そのノードだけを省いて件数を返す。変異（省いた数を 0 と数える）でテストが落ちる。

## Findings
### n1（nit）: 断片の段から、本文をそのまま載せる段が外れている
- 根拠: `fit_or_split` は二分した後、`GROUP_EVIDENCE_TIERS[1..]` から始める。半分なら全文で収まる場合でも、根拠は 200 文字に切り詰められる。
  ADR の削る順序の範囲内で、整理の質への影響は小さい。
- 修正案: 断片にも `GROUP_EVIDENCE_TIERS` の全段を当てる。今のままにするなら doc コメントに書く。

## 判定
converged: yes（Blocker 0 / Major 0 / Minor 0 / nit 1）
