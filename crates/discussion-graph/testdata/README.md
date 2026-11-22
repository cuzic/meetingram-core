# 合成データ

すべて合成した会議と手作業で付けた正解。発話は JSONL、正解は JSON で、根拠は発話の `row_id` を指す。

| データ | ブランチ | 発話と正解の出典 |
| --- | --- | --- |
| m1、m2、m3 | `spike/discussion-graph-s1-measure` | `spike/discussion-graph/meetings/*.json` と `spike/discussion-graph/gold/*.gold.json` |
| budget、design、one-on-one | `spike/discussion-graph` | `spikes/s1-discussion-graph/data/*.jsonl` と `spikes/s1-discussion-graph/gold/*.json` |

出典で状態の注釈が省略されたノードには、ADR の初期状態 `open` を入れた。

## ADR 0001 r13 の親の付け直し

`responds_to` する position が 2 件以上か、position が `objects_to` を受けていれば議題として扱う。それ以外は発話も確認し、親の議題のための事実・数値・可否の確認に親を付けた。

| データ | issue | position 数 | 反対あり | 親 / 判定 | 理由 |
| --- | --- | --- | --- | --- | --- |
| m1 | `g3`（A社への説明） | 1 | なし | 親なし | リリース日の判断材料を確認する問いではなく、延期を受けて顧客への伝え方を決める問い。 |
| m1 | `g5`（リリースノートの書き方） | 1 | あり | 親なし | position に反対があり、案が対立している。 |
| m2 | `g4`（来期の目標） | 1 | なし | 親なし | キャリアの方向性を踏まえて別途設定する目標の相談。 |
| m3 | `g3`（ベンダーロックイン） | 1 | なし | 議題（最終の規則） | 収集方式のルールを決める問いであり、事実・数値・可否の確認に当たらない。 |
| m3 | `g4`（年間予算の上限） | 0 | なし | `g1`（乗り換え先） | 候補が年間700万円に収まるかの確認。 |
| budget | 全 issue | 各 2 | 問わない | 親なし | 全件が position の件数で議題の条件を満たす。 |
| design | `i1` / ほかの issue | 1 / 各 2 | `i1` はあり | 親なし | `i1` は反対、ほかは position の件数で条件を満たす。 |
| one-on-one | `i3`（発表前の読み合わせ） | 1 | なし | 保留 | 資料の読み合わせを依頼する問いであり、発表の説明範囲を決めるための事実・数値・可否の確認に当たるかは明確でない。 |
| one-on-one | `i2` | 1 | あり | 親なし | position に反対があり、議題の条件を満たす。 |
