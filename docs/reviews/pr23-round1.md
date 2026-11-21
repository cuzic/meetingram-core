# PR #23 レビュー（round 1）

対象: cuzic/meetingram-core fix/cli-temp-base @ 617fbba4c93452dac5daf642edc6583d8af6a845

## 結論（先に）
converged: yes。呼び出しごとのディレクトリは `<base>/<pid>-<開始時刻>/meetingram-cli-*` に作られ、起動時の掃除は自分と生きているプロセスの下位ディレクトリを残し、
死んだプロセスと PID が再利用されたもの（開始時刻の不一致）を消す。Linux（`/proc/<pid>/stat` の 22 番目）・macOS（`proc_pidinfo` の開始時刻）・
Windows（`GetProcessTimes` の作成時刻）の 3 つとも開始時刻で再利用を見分け、3 つの OS の CI で生きている・終わった・再利用の場合を確かめている。
Rhai の既定（`RhaiEngine::new` → `None`）は変わらない。残りは、開始時刻を読めない「生きている」の分岐にテストが無い点などの Minor 1 つと nit 2 つ。

## 前ラウンドの解消状況
なし（round 1）。

## Findings
### m1（Minor）: `ProcessStart::Alive`（生きているが開始時刻を読めない）の分岐にテストが無く、この場合はいつまでも消さない
- 根拠: `cli_backend.rs` の `is_stale_cli_dir` の `ProcessStart::Alive => return Ok(false)` を `Ok(true)` に変える変異が全テストを通る（生きているプロセスの作業中のディレクトリを消す方向の変異）。
  Unix の `process_start` は `kill(pid, 0)` が `EPERM`（他ユーザのプロセス）か開始時刻を読めないときに `Alive` を返す。その場合は 1 時間の経過でも消さないので、
  死んだプロセスの PID が他ユーザの長寿命のプロセスに再利用されると、残りがそのプロセスの寿命まで残る（漏れるだけで、消しすぎは無い）。
- 修正案: Unix で `pid 1`（CI の一般ユーザでは `EPERM`）の名前の下位ディレクトリが残るテストを足す。`Alive` でも開始時刻が記録されていない（`t` 付きの名前）か
  確かめられないものは、最終更新から十分古い（例: 24 時間）ものだけ消すことを検討する。

### n1（nit）: 1 つの項目でのエラー（`NotFound` 以外）で、残りの項目の掃除をやめる
- 根拠: `cleanup_stale_cli_dirs` の `other => other?`。例えば 1 つの下位ディレクトリの `remove_dir_all` が `PermissionDenied` や、別のプロセスが中に作った直後の
  `DirectoryNotEmpty` で失敗すると、後ろの項目が残る。アプリは種類を `warn` に出して続けるだけなので影響は残りが増えることだけ（従来と同じ振る舞い）。
- 修正案: 項目ごとのエラーは覚えておいて最後に 1 つ返し、残りの項目の掃除は続ける。

### n2（nit）: 自分のディレクトリを名前で飛ばす処理（`name == process_dir_name()`）は、外しても全テストが通る
- 根拠: 自分の開始時刻は `Known` で一致するので、`is_stale_cli_dir` でも残る（二重の守り）。名前の飛ばしが効くのは自分の開始時刻を読めず `t` 付きの名前になった場合だけで、そこはテストされていない。
- 修正案: 現状のままでよい（二重の守りとしてコメントを 1 行足す程度）。

## 確かめたこと
- PID の再利用: Linux は `rsplit_once(')')` の後の 20 番目（= 22 番目の starttime、起動からの tick）で、コマンド名の空白・括弧にも強い。macOS は
  `pbi_start_tvsec * 1e6 + pbi_start_tvusec`、`proc_pidinfo` の戻り値が構造体の大きさのときだけ `assume_init`。Windows は
  `OpenProcess(QUERY_LIMITED_INFORMATION | SYNCHRONIZE)`、`ERROR_INVALID_PARAMETER` → 死、`ACCESS_DENIED` → 生、待ちが成立 → 死（ハンドルが残る終わったプロセス）、
  ハンドルは `OwnedHandle` で閉じる。自分の名前と他のプロセスから見た名前は同じ API の同じ値なので一致する。
- 自分の作業中のディレクトリ: 名前の一致と開始時刻の一致の二重で残る。同じプロセスの並行の呼び出しは `create_dir` の `AlreadyExists` を許す（シンボリックリンクは拒否）。
- 掃除と新しい呼び出しの競合: 消すのは死んだプロセス・開始時刻の違うプロセス・確かめられず 1 時間更新の無いものだけで、新しい呼び出しは自分の（生きている）下位ディレクトリにしか作らないので重ならない。
  2 つのインスタンスが同じ残りを同時に消す場合の `NotFound` は無視される。旧形式（基底の直下の `meetingram-cli-*`、v0.3.0 のアプリ）は 1 時間より古いものだけ消すので、
  動いている v0.3.0 の呼び出し（上限 600 秒）は消さない。
- Rhai: `RhaiEngine::new` は `new_with_cli_temp_base(..., None)` を呼ぶだけで既定は不変。`dispatcher` は非公開のモジュールなので `async_worker` の引数の追加は公開 API ではない。
  公開 API の追加は `RhaiEngine::new_with_cli_temp_base` だけ（PR 本文に記載）。`cleanup_stale_cli_dirs` の意味が「旧形式も直ちに消す」から「1 時間より古いものだけ」に変わった点も本文の記述どおり。
- 変異: 生きているプロセスのディレクトリを消す、開始時刻を無視する（再利用を見ない）、死んだものを残す、旧形式の経過時間を見ない、Rhai が基底を渡さない、Unix の `ESRCH` を生とする、はすべて落ちた。
  生き残ったのは m1 と n2 の 2 つ。
- 完了の定義（Linux）: fmt、workspace の clippy、`cargo test`（失敗 0）、`capture-windows` の Windows 向け clippy、すべて pass。
  `gh pr checks 23` は 5 件すべて pass。ジョブログで `cleanup_preserves_live_process_and_removes_its_dir_after_exit` が Linux・Windows・macOS で ok、
  `ai_summarize_uses_configured_cli_temp_base` が Linux で ok（`cfg(unix)` のテストで、macOS の CI は rhai-engine のテストを回さないので Linux だけ）。

## 判定
converged: yes（Blocker 0 / Major 0 / Minor 1 / nit 2）
