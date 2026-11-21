# PR #13 レビュー（round 1）

対象: cuzic/meetingram-core feat/summarize-cli-stdin @ 39fcf998f3ad2633380e4bc230a78bd40d2736e9

## 結論（先に）
converged: no。本文を argv に載せない・stderr を保持しない・一時ディレクトリの後始末・Unix / Windows のツリーの停止・claude の lean と互換は、
テスト・CI・実 CLI（claude 2.1.330 で lean が通り互換に落ちない）で確かめられた。
しかし実 codex（0.167.0）では、ADR の `features.code_mode_host=false` が `item.completed` の `error` 項目を毎回出し、許す一覧に無いので
**codex の要約が常に `tool_use` で失敗する**（M1、ADR の改訂が要る）。また今の要約の経路に、これまで無かった 60 秒の上限が入った（M2）。

## 前ラウンドの解消状況
なし（round 1）。

## Findings
### M1（Major）: 実 codex 0.167.0 では codex の要約が毎回 `ToolUse` で失敗する
- 根拠: PR の経路（`summarize_via_cli_with_limits(CliBackend::Codex, …)`、合成の 1 行）を実 codex で呼ぶと 148 ms で `Err(ToolUse(Codex))`。
  同じ引数で直接呼んだ `--json` の出力は、`turn.started` より前に
  `{"type":"item.completed","item":{"type":"error","message":"Code Mode is unavailable because code-mode host is disabled. …"}}` を出す
  （その後の `agent_message` と `-o` の応答は正常）。`-c features.code_mode_host=false` を外すとこの項目は出ない。
  `crates/summarize/src/cli_backend.rs:456` の許す一覧（`agent_message`・`reasoning`）は ADR §2 の文言どおりだが、ADR の 2 つの決定
  （`code_mode_host` の無効化と許す一覧）が実 CLI では両立しない。PR 本文は codex を「未確認」としていた。
- 修正案: owner が ADR §2 を改訂して決める。推奨は「`item.type = error` はツールの呼び出しではないので許す（止めない）」で、
  `code_mode_host` の無効化は残す。決まったら一覧に足し、`error` 項目を先頭に出すダミーの codex で要約が返るテストを足す。

### M2（Major）: 今の要約（`CliSummarizer`、Rhai の hint）に 60 秒の上限が新しく入り、長い会議の要約が `timeout` で落ちうる
- 根拠: `crates/summarize/src/lib.rs:207` → `summarize_via_cli`（`cli_backend.rs:155`）が `DEFAULT_CLI_TIMEOUT`（`:48`、60 秒）を使う。
  `origin/main` の CLI 経路には時間の上限が無かった。ADR §7 の 60 秒は**グラフの増分**の値で、整理（1 万〜1.2 万文字）でも 120 秒を置き、
  S1 で 17〜45 秒かかっている。1 時間の会議の全文要約や推論の長い codex では 60 秒を超えうる。
  `Summarizer` の経路からは上限を変える手段が無い（アプリはタグを上げるだけでこの挙動になる）。
- 修正案: `summarize_via_cli` は要約用の長い既定（例: 300 秒、別の定数）か上限なしにし、60 秒はグラフの呼び出しが
  `summarize_via_cli_with_limits` で渡す。値は owner が決める。

### m1（Minor）: 互換への切り替えの条件の一部にテストが効いていない
- 根拠: 変異 2 つが全テストを通った。(a) `cli_backend.rs:250` の `&& !result.status.success()` を消す（lean が成功しても stderr に文言があれば
  やり直す＝費用 2 倍）。(b) `:489` の改行での照合のリセットを消す（行の途中の `error: unknown option '` でも当たる）。
  `compatibility.store` を消す、`process_group(0)` を消す、許す一覧を任意にする、文言の検出を止める、書き込みエラーを常に致命にする、の 5 つは落ちた。
- 修正案: lean が 0 で終わり stderr に文言を出すダミーで再試行しないこと、行の途中の文言では切り替えないことのテストを足す。

### m2（Minor）: Windows のパイプが作成直後は両端とも継承可能で、他スレッドの `std::process::Command` の起動に紛れ込みうる
- 根拠: `windows.rs:127` で `bInheritHandle: 1` で作り、`:142` で親側だけ後から外す。子側は `spawn` の終わりまで継承可能。
  自前の起動は `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` で守られるが、アプリの他の起動（ハンドルの一覧なしで `bInheritHandles = TRUE`）が
  この間に走ると、その子が stdout の書き側や stdin の書き側を持ち続け、EOF が来ずに要約が `timeout` になる。
- 修正案: 継承不可で作り、`CreateProcessW` の直前に子側だけ `DuplicateHandle` で継承可能な複製を作って直後に閉じる（窓を最小にする）。

### m3（Minor）: Windows の `resolve` が `PATH` の空要素・相対要素をカレントディレクトリとして探す
- 根拠: `windows.rs:18` の `split_paths` は `a;;b` の空要素を返し、`Path::new("").join("claude.exe")` はカレントの `claude.exe` になる
  （カレントに置かれた実行ファイルを拾う）。
- 修正案: 空・相対の要素を飛ばす。

### m4（Minor）: Unix の停止は `killpg` を送るだけで、止まり終わりを待たずに一時ディレクトリを消す
- 根拠: `unix.rs:17`。ADR §2「止め終わってから一時ディレクトリを消す」。SIGKILL 直後に孫が書いていると `remove_dir_all` が失敗し、
  `TempDir` は黙って残す（次回の `cleanup_stale_cli_dirs` まで残る）。また先頭のプロセスを回収した後の `killpg` は、
  グループが空なら再利用された PGID に当たりうる（確率は低い）。Windows 側は Job の終了を待っている。
- 修正案: `kill(-pgid, 0)` が `ESRCH` になるまで短く待つ（上限付き）。

### m5（Minor）: Windows CI のテストが `.exe` を `.cmd` より先に選ぶこと・`PATH` の順を確かめていない
- 根拠: `tests.rs` の Windows テストは `claude.cmd` だけを置く。同じディレクトリに `.exe` と `.cmd`、前のディレクトリに `.cmd`・後ろに `.exe` の場合が無い。
- 修正案: `resolve` の順序のテストを足す（`is_file` だけなので Windows 上で空ファイルで足りる）。

### n1（nit）: Windows の `Group::drop`（`WaitForSingleObject(INFINITE)` と Job の監視ループ）と `WorkingDir::drop`（最大 100 ms の sleep）が tokio のワーカーを塞ぐ
- 根拠: `windows.rs:215` 以降、`cli_backend.rs` の `impl Drop for WorkingDir`。通常は短いが、終わらないプロセスがあると無期限。
- 修正案: 待ちに上限を付けるか、`spawn_blocking` に移す。

## 確かめたこと
- 完了の定義（Linux）: `cargo fmt --all --check`、workspace の clippy、`cargo test`（49 バイナリ全 ok）、`capture-windows` の Windows 向け clippy、すべて pass。
- `gh pr checks 13`: Linux・Windows・macOS の build と e2e がすべて pass。Windows のジョブログで `windows_resolves_batch_shim_and_stops_its_descendants ... ok`
  （`.cmd` の解決と `is_available`、改行入り 33,000 文字の stdin、空の引数、空白・日本語のパス、`.cmd` 経由の孫のタイムアウトと drop）を確認。
  `windows-build.yml` の `paths`（`crates/**`）は summarize を覆う。
- 実 CLI（合成の 1 行）: claude 2.1.330 の lean は 4.1 秒で成功、`claude_compatibility_mode() == false`、一時ディレクトリは残らない。
  `--system-prompt-file` と codex の `model_instructions_file`（相対名）が効くことを、指示どおりの単語を返させて確認。
  未知のオプションの文言は `error: unknown option '--…'`（終了コード 1）で定数と一致。codex は M1。
- 本文は argv・エラー・ログに出ない（引数の列のテストと、stderr の目印のテスト。crate にログ出力なし）。
- アプリ（`~/src/meetingram` の `origin/main`）は `CliSummarizeError` の変種を参照しておらず、タグを上げてもビルドは壊れない（M2 の挙動の変化はある）。

## 判定
converged: no（Blocker 0 / Major 2 / Minor 5 / nit 1）
