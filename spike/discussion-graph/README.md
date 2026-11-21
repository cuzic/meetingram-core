# Spike S1 — meetingram ADR 0001（議論グラフ）の計測

このブランチ（`spike/discussion-graph`）はマージしない。結果は meetingram の ADR 0001 r5 に書き写した。

## 中身
- `harness.py`: ADR r4 の設計どおりの増分抽出（6 件／45 秒、文脈の枠、§2 の検証の規則、流し切り、整理）と、正解との比較。stdlib だけ。
- `meetings/m{1,2,3}.json`, `gold/m{1,2,3}.gold.json`: 合成の日本語の会議（架空）と、手で作った正解の IBIS。`gen/` が生成スクリプト。
  - m1 リリース判断 168 発話（決定と宿題が最後の 4 発話）、m2 1on1 173 発話（1,478 文字の独白）、m3 ツール選定 164 発話（時刻なし）。
- `argv-probe/` と `.github/workflows/spike-argv-windows.yml`: Windows で Rust の `Command` がどれだけ長いプロンプトを渡せるか。
- `results/`: 各実行の結果（`*.json`）、呼び出しごとの記録（`*.log`）、評価（`*.eval.txt`）。

## 再現
```sh
./run-matrix.sh                       # claude CLI と codex CLI にログインした環境で。並列で約 30 分
python3 -I harness.py run --meeting meetings/m3.json --backend claude --model haiku --ctx-all --out results/x.json
python3 -I harness.py eval --result results/x.json --gold gold/m3.gold.json
```
ノードの一致は「種類が同じで、根拠の発話（row_id）の集合の Jaccard が最大のものを 1 対 1 で取る」。エッジは一致したノード同士で (src, dst, kind) を比べる。

## 結果（2026-11-21、claude 2.1.330 / codex-cli 0.167.0）

| 実行 | ノード P / R / F1 | 重複 | エッジ F1 | 呼び出し | 読めた割合 | p50 / p95 秒 | 入力トークン（新規＋キャッシュ作成＋読み） | 出力 | 費用（CLI 表示） |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| m1 sonnet | 0.79 / 0.96 / 0.86 | 4 | 0.59 | 29 | 1.00 | 7.3 / 13.2 | 58 + 297k + 263k | 19k | $1.41 |
| m1 haiku | 0.68 / 0.91 / 0.78 | 3 | 0.54 | 29 | 1.00 | 12.3 / 18.0 | 58 + 294k + 288k | 60k | $0.092 |
| m1 haiku lean | 0.74 / 0.94 / 0.83 | 4 | 0.60 | 29 | 1.00 | 8.9 / 13.2 | 58 + 41k + 24k | 40k | $0.028 |
| m1 haiku 12 件／90 秒 | 0.71 / 0.89 / 0.79 | 5 | 0.51 | 15 | 1.00 | 16.2 / 22.4 | 30 + 159k + 149k | 44k | $0.056 |
| m1 haiku 文脈に全ノード | 0.69 / 0.94 / 0.80 | 6 | 0.49 | 29 | 1.00 | — / 19.1 | 58 + 300k + 290k | 66k | $0.096 |
| m1 codex（既定モデル） | 0.70 / 0.98 / 0.82 | 6 | 0.57 | 30 | 0.97 | 11.9 / 16.0 | 534k（うちキャッシュ 415k） | 10k | — |
| m2 sonnet | 0.60 / 0.88 / 0.72 | 6 | 0.32 | 31 | 1.00 | 7.0 / 12.6 | 62 + 312k + 281k | 18k | $1.46 |
| m2 haiku | 0.46 / 0.91 / 0.61 | 14 | 0.14 | 31 | 1.00 | 11.3 / 17.2 | 62 + 317k + 308k | 60k | $0.097 |
| m2 haiku 文脈に全ノード | 0.58 / 0.88 / 0.70 | 10 | 0.20 | 31 | 1.00 | — / 18.0 | 62 + 318k + 312k | 53k | $0.094 |
| m3 sonnet | 0.62 / 0.91 / 0.74 | 9 | 0.51 | 29 | 1.00 | 7.9 / 11.6 | 58 + 298k + 263k | 22k | $1.44 |
| m3 haiku | 0.44 / 0.91 / 0.59 | 23 | 0.29 | 29 | 1.00 | 12.1 / 19.4 | 58 + 300k + 288k | 67k | $0.096 |
| m3 haiku 文脈に全ノード | 0.46 / 0.88 / 0.60 | 16 | 0.26 | 29 | 1.00 | — / 20.0 | 60 + 311k + 310k | 61k | $0.096 |

- 会議の最後の 4 発話にある決定・宿題は、全実行で全部取れた（m1 5/5、m2 3/3、m3 2/2）。
- 検証で落ちた操作は 0〜3 件／114〜171 件で、すべて「同じエッジの重複」。
- 増分 1 回の入力は中央値 2.1k 文字、最大 3.1k 文字。整理は 10〜12k 文字、17〜45 秒。
- 「lean」は claude CLI に `--tools "" --strict-mcp-config --setting-sources "" --system-prompt <system>` を付け、ユーザー部分だけを渡したもの。
  通常の呼び方は Claude Code 自身のシステムプロンプトで 1 回約 1 万トークンを毎回キャッシュ作成する（作業ディレクトリが毎回違うためキャッシュに当たらない）。

### Windows での引数（GitHub Actions windows-latest、Rust の `tokio::process::Command`）
| 渡し方 | 結果 |
| --- | --- |
| .exe に引数で | 28,000 文字まで通る、32,000 文字で `os error 206` |
| npm の .cmd シムに引数で | 改行を含むと 1,000 文字でも `InvalidInput: batch file arguments are invalid` |
| .exe に標準入力で | 100,000 文字でも通る |
| `Command::new("claude" / "codex")`（拡張子なし） | npm で入れた `claude.cmd` / `codex.cmd` は NotFound。ネイティブの `claude.exe` だけ見つかる |

Linux では 1 引数の上限（128 KiB）で、日本語は約 4.3 万文字で `ArgumentListTooLong`。claude と codex はどちらも標準入力でプロンプトを受け取れる（`claude --print`、`codex exec -`）。
