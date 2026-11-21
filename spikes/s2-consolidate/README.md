# ADR 0001 S2 整理の計測

[Issue #18](https://github.com/cuzic/meetingram-core/issues/18) のマージしない spike。コアの合成会議 6 本と、架空の展示会の運営を話し合う 60 分・600 発話の `long` を使う。`long.gold.json` の `duplicate_issue_pairs` は、同じ問いを表す前半と後半の正解ノード ID の組。

## fake で再現

```sh
cd spikes/s2-consolidate
cargo test --locked
cargo run --locked -- run --backend fake --out results/fake
cargo run --locked -- report --out results/fake
```

fake は最初の根拠を含む窓で正解のノードを返し、両端がそろった時点でエッジを返す。整理には空の操作列を返す。コアのプロンプト生成・応答解析・検証・評価を通し、全 42 行で再現率 1.000 を確認できる。

## Claude が sandbox の外で実行

認証済みの `claude` が PATH にある環境で実行する。

```sh
cd spikes/s2-consolidate
./run-matrix.sh results/s2
```

スクリプトは 3 通りの呼び出し数の見積もりを最初に表示する。sonnet と haiku の各 7 本を 6 件／45 秒の窓で流し切り、両方式の整理を各 3 回実行する。続いて sonnet の短い 6 本を 12 件／90 秒の窓で流し切り、強めた整理を 1 回実行する。見積もりは発話窓と分割前の整理の回数を足した値。分割、プロンプトの上限で窓に入りきらなかった発話、再試行によって実際の回数は増える。`MAX_CALLS` で実行ごとの上限を変更できる（既定 700）。各実行の途中で上限や失敗に達した場合は、その時点までの結果を集計してスクリプトが停止する。

単独で実行する場合は新しい出力ディレクトリを指定する。

```sh
cargo run --locked -- estimate --backend claude-cli --model sonnet
cargo run --locked -- run --backend claude-cli --model sonnet --out results/sonnet
cargo run --locked -- report --out results/sonnet
```

## 出力と判定

- 各会議の `flush.json` を起点として読み戻し、毎回同じグラフから整理を始める。解析できた応答はコアで操作ごとに検証する。抽出で連続 3 回失敗した会議は整理を省く。分割整理で 1 回失敗した試行は打ち切り、流し切りのグラフへ戻す。
- `calls.jsonl` は呼び出しごとの経過時間、入出力の文字数、解析の可否、検証で落とした操作数、エラー種別を記録する。CLI の作業場所は OS の一時ディレクトリに作り、終了時に削除する。グラフ JSON には合成会議の発話とノードの本文が含まれる。
- `RESULTS.md` は再現率、適合率、エッジ F1、誤 merge 数、正解の重複組の統合数、整理時間、タイムアウト数、分割数を試行ごとに表示する。重複組は整理前の別々の予測 issue を根拠で正解に対応付け、整理後の統合先が一致した組を数える。コアの誤 merge 指標では、長い会議で重複として注釈した別 ID 間の統合も誤 merge に含まれる。
- タスク 4b の判定には sonnet の既定窓の計測を使う。短い 6 本で会議ごとの 3 回の中央値を比較し、ノード数の削減率を会議間で同じ重みで平均する。再現率は各会議で中央値の低下があるかを調べる。長い会議は 3 回のうち 2 回以上で分割を通り、正解の組が 1 組以上統合されれば条件を満たす。試行が欠けた場合は判定保留とする。
