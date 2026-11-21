# ADR 0001 S1 議論グラフ抽出の計測

この Rust ワークスペースには合成会議、手作業で付けた正解、抽出の再生と評価、Windows の引数長プローブがある。

Linux の確認はこのディレクトリで実行する。

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
./run-matrix.sh fake design
```

Claude が sandbox の外で認証済みの両 CLI を用意し、以下を実行する。

```sh
./run-matrix.sh live
```

三会議で 390 回を予定する。スクリプトは各設定の窓数を先に算出し、再試行と入力分割も含む実呼び出しを 400 回までに制限する。入力が収まらない場合は同じ窓の残りを次の呼び出しに回す。失敗した入力は次の窓に持ち越す。最後まで失敗した発話はグラフに反映されない。

出力は既定で `runs/<UTC 時刻>/` に書く。各実行の `calls.jsonl` には生の応答、解析結果、操作、計測値が入る。各実行の `graph.json` は最終グラフ、`score.json` は正解との比較、`RESULTS.md` はマトリクスの集計。実行結果は合成会議のデータを含むので、他の入力に転用する際は保存先に注意する。

単独実行の設定は `cargo run --locked -- run --help` で確認できる。既存の複数の結果を集計し直すには `cargo run --locked -- aggregate --out /tmp/RESULTS.md <run-dir>...` を使う。

Windows の測定は `spike-s1-windows` の job summary に出る。前置きの引数を含め、BMP の「会」と ASCII の `a` で、末尾の引数が指定の文字数どおり届く上限を測る。引用符や改行を含む実プロンプトでは、エスケープで使える文字数が変わる。CLI の導入後は認証不要の `--version` で名前解決を確かめる。
