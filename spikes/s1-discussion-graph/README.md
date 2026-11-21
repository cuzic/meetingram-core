# ADR 0001 S1 議論グラフ抽出の計測

この Rust ワークスペースには合成会議、手作業で付けた正解、抽出の再生と評価、Windows の引数長プローブがある。

Linux の確認はこのディレクトリで実行する。

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
./run-matrix.sh fake
```

Claude が sandbox の外で認証済みの両 CLI を用意し、以下を実行する。

```sh
./run-matrix.sh live
```

マトリクスは `--cli-mode lean` で実行し、三会議の基本呼び出しは 378 回。スクリプトは各設定の窓数を先に算出し、再試行と入力分割も含む実呼び出しを 400 回までに制限する。入力が収まらない場合は同じ窓の残りを次の呼び出しに回す。失敗した入力は次の窓に持ち越す。最後まで失敗した発話はグラフに反映されない。

出力は既定で `runs/<UTC 時刻>/` に書く。各実行の `calls.jsonl` には`cli_mode`、生の応答、解析結果、操作、計測値が入る。各実行の `graph.json` は最終グラフ、`score.json` は正解との比較、`RESULTS.md` はマトリクスの集計（`cli-mode`、会議の長さ、1 時間あたりの呼び出し・入出力トークンを含む）。過去の結果では `cli-mode` は「—」と表示する。実行結果は合成会議のデータを含むので、他の入力に転用する際は保存先に注意する。

単独実行の設定は `cargo run --locked -- run --help` で確認できる。既存の複数の結果を集計し直すには `cargo run --locked -- aggregate --out /tmp/RESULTS.md <run-dir>...` を使う。

## CLI の計測モード

`run` の既定は `lean`。抽出の規則（`src/prompt.txt`）を system に、文脈・直前の発話・新しい発話を user に渡す。入力上限は system と user を合わせて数える。旧方式を再現する `full` では、従来どおり改行で結合して 1 つの引数に渡す。

- Claude の `lean` は `--tools "" --strict-mcp-config --setting-sources "" --system-prompt <system>` を使う。OAuth ログインで計測する。
- Codex の `lean` は `--ignore-user-config` でユーザーの `config.toml` を外し、既存の認証を使う。`model_instructions_file` に作業用一時ディレクトリの system ファイルを指定し、`project_doc_max_bytes=0` と空の作業ディレクトリで `AGENTS.md` の読み込みを抑える。`mcp_servers={}`、`features.shell_tool=false`、`features.view_image=false`、`web_search="disabled"` も指定する。全ツールをまとめて外すフラグは `codex exec --help` と設定リファレンスで確認できなかった。管理側の設定や残る組み込みツールの影響は、認証済み CLI で実測する。[CLI リファレンス](https://developers.openai.com/codex/cli/reference) · [設定リファレンス](https://developers.openai.com/codex/config-reference)

合成の設計会議の最初の窓（6 発話または 45 秒）を 1 回ずつ呼ぶ。各コマンドは入出力トークンと応答の先頭 200 文字を表示する。

```sh
cargo run --locked -- probe --backend claude --model sonnet --cli-mode lean --input data/design.jsonl
cargo run --locked -- probe --backend claude --model sonnet --cli-mode full --input data/design.jsonl
cargo run --locked -- probe --backend codex --cli-mode lean --input data/design.jsonl
cargo run --locked -- probe --backend codex --cli-mode full --input data/design.jsonl
```

Windows の測定は `spike-s1-windows` の job summary に出る。前置きの引数を含め、BMP の「会」と ASCII の `a` で、末尾の引数が指定の文字数どおり届く上限を測る。引用符や改行を含む実プロンプトでは、エスケープで使える文字数が変わる。CLI の導入後は認証不要の `--version` で名前解決を確かめる。
