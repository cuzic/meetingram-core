# PR #17 レビュー（round 1）

対象: cuzic/meetingram-core feat/summarize-complete @ 5cd0546c1a08f7e270d3948ab075ab3d9b1519ae

## 結論（先に）
converged: yes。`SummaryProvider` の 13 種類の鍵・既定のモデル・`api_key_account`・`is_vertex`・`cli_backend` はアプリ（cuzic/meetingram main の
`apps/desktop/src/settings.rs`）と一致し、組み立ては `summary_consumer.rs` の分岐と同じ。`complete` の CLI 版は PR #13 の経路（system はファイル、
user は標準入力、時間の上限、ツリーの停止）をそのまま通り、実 claude と実 codex の両方で返る。アプリに `Summarizer` の実装は無く、必須メソッドの追加でアプリは壊れない。
残りは、`complete` の一時ディレクトリをアプリのデータディレクトリに向けられない点（Minor、タスク 5 の前に）と nit 1 つ。

## 前ラウンドの解消状況
なし（round 1）。PR #13 round 2 の follow-up:
- m1: 解消。`summarize_enforces_its_default_and_explicit_deadlines`（時間を止めた実経路）は、要約の経路を 60 秒の定数に戻す変異で落ちる。
  stderr の 2 行目の文言で互換に切り替わるテストは、改行でのリセットを消す変異で落ちる。
- m2: 解消。`macos-build.yml` が `cargo test -p summarize --target aarch64-apple-darwin` を回し、macOS のジョブログで 33 件 ok
  （`timeout_and_drop_stop_grandchild…`、`unix_stops_descendants_when_the_leader_exits` を含む）。`paths` の `crates/**` が覆う。

## Findings
### m1（Minor）: `complete` の CLI 版の一時ディレクトリが常に OS の一時ディレクトリで、ADR §2 の「アプリのデータディレクトリの下」にできない
- 根拠: `cli_backend.rs` の `complete_with_binary` は `run_named_cli(…, None, req.timeout)` を呼び、`CompletionRequest` にも `CliSummarizer` にも
  `build_summarizer` にも基底ディレクトリを渡す口が無い。ADR §2 は一時ディレクトリをデータディレクトリの下に作り、次の起動時に
  `cleanup_stale_cli_dirs` で残りを消すとしている。OS の一時ディレクトリに対して `cleanup_stale_cli_dirs` を呼ぶと、同時に動く別の
  meetingram のプロセス（タスク 5 の要約とグラフの並行）の作業中のディレクトリまで消しうる。仕様（#16）の型の定義どおりなので、仕様の穴。
- 修正案: タスク 5 の前に、`build_summarizer` か `CliSummarizer` に CLI の基底ディレクトリ（`Option<PathBuf>`）を持たせ、`summarize` と `complete` の両方で使う。

### n1（nit）: API キーのクライアントにモデルの行き先の制限（`with_model_mapper_fn`）を足したのは仕様と ADR に無い判断
- 根拠: `provider.rs` の `build_api_key_client`。アプリの今の組み立てには無い。アプリの `known_models` の全 23 件（API キーの 7 プロバイダ）と
  `o4-mini`・`chatgpt-4o-latest` は新旧どちらでも解決できることを確かめた。旧来は通っていた `ft:gpt-4o-mini:org::abc` の形は拒否される
  （旧来も OpenAI には届かない形なので実害は無い見込み）。キーを別のプロバイダに送らないための変更で、PR 本文には書いてある。
- 修正案: ADR の次の改訂で一文足す（コードの変更は不要）。

## 確かめたこと
- アプリとの一致: `key`・`default_model`・`api_key_account`・`from_key`（未知は `Claude`）・`is_vertex`・`cli_backend` を 13 種類すべて
  アプリの `settings.rs` と突き合わせて一致。`supports_graph` は Codex だけ偽。
- 組み立て: CLI → `CliSummarizer`、ClaudeOAuth → `build_claude_oauth_client`、Vertex（読めなければ `MissingVertexCredentials`、JSON が不正なら
  `InvalidVertexCredentials`）、Ollama（基底 URL）、API キー（読めなければ `MissingApiKey`）、その他は既定のクライアント。アプリの分岐と同じ順と結果。
- Rhai: 既定の鍵（未設定 → `Claude`）は従来どおり。変化は、ClaudeOAuth が組み立てられる、API キー・Vertex の未設定が呼び出し前に設定のメッセージになる、
  genai のプロバイダ失敗の文言が固定文になる（本文を含みうる `genai::Error` の Display を出さない）の 3 点で、どれも PR 本文に記載。
- エラー文: `complete` の genai 失敗は `CompletionProvider`（固定文）、認証の解決の失敗は `credential store load failed`（キーリングのエラー文も出さない）、
  `ant` の失敗は終了コードだけ（stderr を捨てる）。`CompletionRequest` は `Debug` を持たない。
- 入力の上限: system＋user の Unicode 文字数で 24,000 まで（超えたら `InputTooLong`、genai と CLI の両方で送る前）。境界の変異（24,001）で落ちる。
- 変異（すべて落ちた）: `supports_graph` を常に真、`resolve_model` の上書きを無視、要約の上限を 60 秒、改行でのリセットを削除、上限を 24,001、
  CLI の `complete` で検証を外す、CLI の `complete` が `req.timeout` を無視、モデルの行き先の制限を外す、API キーの事前確認を外す。
- 完了の定義（Linux）: fmt、workspace の clippy、`cargo test`（失敗 0）、`capture-windows` の Windows 向け clippy、すべて pass。
  `gh pr checks 17`: Linux・Windows・macOS の build と e2e がすべて pass（head 5cd0546）。Windows のジョブログで `.cmd` の解決・孫の停止・パイプの継承のテストが ok。
- 実 CLI（合成の 1 行、sandbox の外、`CliSummarizer(..).complete`、system は「PINEAPPLE とだけ返す」）: claude 2.1.330 は 2.3 秒で `PINEAPPLE`
  （互換に落ちない）、codex 0.167.0 は 4.2 秒で `PINEAPPLE`。
- 公開 API: `Summarizer::complete` は必須だが、アプリ（main）に `Summarizer` の実装は無い（`GenaiSummarizer` と `CliSummarizer` だけ）。
  アプリは `use summarize::{Summarizer, TranscriptTurn}` だけで、自前の `SummaryProvider` と名前は衝突しない。

## 判定
converged: yes（Blocker 0 / Major 0 / Minor 1 / nit 1）
