# PR #13 レビュー（round 2）

対象: cuzic/meetingram-core feat/summarize-cli-stdin @ 7786af0748263cdf0fc93467ca16eb2a4303235a

## 結論（先に）
converged: yes。round 1 の M1・M2 は ADR r10 どおりに直り、実 codex 0.167.0 の要約が PR の経路で返るようになった（実 claude の lean も引き続き動く）。
m1〜m5、n1 も挙動とテストで解消を確かめた。39fcf99 からの差分に新しい Blocker / Major は無い。
残りはテストの穴と、macOS で summarize をビルドもテストもしていない点（Minor）で、follow-up でよい。

## 前ラウンドの解消状況
- M1: 解消。許す項目が `agent_message`・`reasoning`・`error`（ADR r10 §2）。`codex_allows_code_mode_error_but_requires_a_response` は `error` を外す変異で落ちる。実 codex 0.167.0 で要約が返る（下記）。
- M2: 解消。`summarize_via_cli` は `SummarizeOptions::with_cli_timeout` か既定の 600 秒（ADR r10 §7）、`DEFAULT_CLI_TIMEOUT`（60 秒）はグラフ用。ただし経路のテストは定数の比較だけ（m1 下記）。
- m1: 解消。lean が 0 で終わったら再試行しない（`&& !result.status.success()` を消す変異で落ちる）。行の途中の文言では切り替えない（`embedded ` の行で確認）。
- m2: 解消。パイプは継承不可で作り、`CreateProcessW` の直前に子側だけ継承可能な複製を作って直後に閉じる。Windows CI の `only_child_pipe_duplicates_are_inheritable` が ok。
- m3: 解消。`resolve_in_path` は絶対パスの要素だけを見る。Windows CI の `windows_fixture_subprocess` で相対の要素と空の要素（カレントに `claude.exe` を置いた状態）を確認。
- m4: 解消。リーダーの終了を `waitid(WNOWAIT)` で見て、回収する前に `killpg` し、停止の後はグループが `ESRCH` になるまで（250 ms 上限）待ってから一時ディレクトリを手放す。グループを待たない変異、リーダー終了時に殺さない変異はどちらも落ちる。
- m5: 解消。`windows_path_search_uses_absolute_directories_in_order`（同じディレクトリで `.exe` が `.cmd` に勝つ、前のディレクトリが後ろに勝つ）が Windows CI で ok。
- n1: 解消。`Group::stop` の待ちと `WorkingDir` の削除は `spawn_blocking`（ランタイムが無ければ専用スレッド）で動く。drop でツリーを止める性質は保たれている（`TerminateJobObject` / `killpg` は同期で送る）。

## Findings
### m1（Minor）: 3 つの変異がテストを通る
- 根拠: (a) `cli_backend.rs` の `options.cli_timeout.unwrap_or(DEFAULT_SUMMARY_TIMEOUT)` を `DEFAULT_CLI_TIMEOUT` に戻しても全テストが通る（`summary_and_graph_have_separate_default_deadlines` は定数を比べるだけ）。
  (b) `read_stderr` の改行での照合のリセットを消しても通る（1 行目に別の出力があり 2 行目に診断がある stderr のテストが無い。消すと互換に切り替わらない）。
  (c) `unix.rs` の `wait` を「先に `child.wait()` で回収してから `killpg`」に戻しても通る（PGID の再利用は決定的に再現できないので、これは許容）。
- 修正案: (a) `with_cli_timeout` の有無で上限が効くことをダミーで確かめる（例: 眠るダミーに短い上限を渡して `timeout`、渡さなければ既定が使われることは関数を分けて確かめる）。(b) 診断の前に別の行を出すダミーで互換に切り替わるテストを足す。

### m2（Minor）: Unix のプロセス管理（`waitid(P_PID, WNOWAIT)`・`killpg`）が macOS でビルドもテストもされていない
- 根拠: `macos-build.yml` は `capture-macos` と `app-service --features macos-supervisor` だけをビルド・テストし、summarize は対象外（ジョブログにも summarize のコンパイルが無い）。
  `libc` 0.2.191 の apple には `waitid`・`P_PID`・`WNOWAIT` があるので型は通る見込みだが、macOS の `waitid` の挙動は未確認。macOS でも CLI の要約は使われる。
- 修正案: `macos-build.yml` に `cargo test -p summarize --target aarch64-apple-darwin` を足す（`paths` は `crates/**` で覆われている）。

## 確かめたこと
- 完了の定義（Linux）: `cargo fmt --all --check`、workspace の clippy、`cargo test`（失敗 0）、`capture-windows` の Windows 向け clippy、すべて pass。
- `gh pr checks 13`: Linux・Windows・macOS の build と e2e がすべて pass（head 7786af0）。Windows のジョブログで
  `windows_resolves_batch_shim_and_stops_its_descendants`、`windows_path_search_uses_absolute_directories_in_order`、
  `only_child_pipe_duplicates_are_inheritable`、`windows_cleanup_runs_off_the_async_worker` が ok。
- 実 CLI（合成の 1 行、sandbox の外、`summarize_via_cli_with_limits` と `summarize_via_cli` の両方）:
  claude 2.1.330 は lean で 4.0 秒・3.8 秒で成功、`claude_compatibility_mode() == false`。codex 0.167.0 は 5.5 秒・3.5 秒で要約が返る（round 1 は `ToolUse`）。
  どちらも `is_available() == true`、一時ディレクトリは残らない。
- 公開 API: `SummarizeOptions` に非公開のフィールドと `with_cli_timeout` を足したが、型は `#[non_exhaustive]` でアプリは `new`／`with_system_prompt` だけを使うので壊れない。

## 判定
converged: yes（Blocker 0 / Major 0 / Minor 2 / nit 0）
