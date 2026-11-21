---
name: implement-task
description: "[codex] の Issue か PR の「## [codex] タスク仕様」コメントを実装し、PR を作る（または更新する）。meetingram / meetingram-core で実装・レビュー指摘の修正・main の取り込みを頼まれたときに使う。設計の判断はしない。"
---

# タスク仕様の実装（Codex）

この skill は meetingram と meetingram-core に同じ内容で置いてある。変えるときは両方を直す。

## 1. 読む

1. 指示された Issue か PR コメントを `gh issue view NN --comments` / `gh pr view NN --comments` で全文読む。
   PR なら最新の `## [codex] タスク仕様` コメントが今回の仕事。
2. 仕様が指す ADR（`docs/adr/`）、レビューファイル（`docs/reviews/prNN-roundK.md`）、
   この repo の `AGENTS.md` の完了の定義を読む。
3. いまのブランチを確かめる（`git status -sb`）。既存 PR なら `git pull --no-rebase origin <branch>`。

## 1.5 git が使えないとき

commit や push が sandbox や権限で失敗したら（`Read-only file system`、`Permission denied`、
`refusing to allow an OAuth App …`、`rejected` など）:

- **回避しない**。別の場所に clone し直す、remote の URL や認証を変える、`.git` を作り直す、はしない。
- 作業はこの作業ツリーに残す（commit できなければ未コミットのままでよい）。テストと確認は普段どおり回す。
- 最後のメッセージの先頭に `Status: blocked（git）`、失敗したコマンド、エラー文をそのまま書いて止める。
  Claude が同じ clone から commit・push・PR 作成を代行する。

## 2. 決めない

仕様に無い判断（公開 API の形、外部サービスの選択、保存形式、UI の文言の方針など）が要ったら、
実装で決めずに、そこまでの作業をコミットして push し、PR 本文の「Open questions」に
選択肢と推奨を書いて止める。「やらないこと」と「触ってよいファイル」の外には手を出さない。

## 3. 作る

- 先に、仕様の受け入れ条件を表すテストを書いて落ちることを確かめ、それから実装する。
- コミットは小さく、意味のまとまりごとに。レビュー指摘の修正はメッセージに指摘 ID（`M1` など）を書く。
- 録音・文字起こしの経路では、後段（要約・ヒント・図）の失敗で録音を止めない。async の中でブロックしない。
- API キー、文字起こし、要約の本文をログやテストに出さない。テストのデータは合成する。
- 一時ファイルとプローブは `$TMPDIR`（`mktemp -d` など）に置く。`/tmp/…` を直接書かない。repo の中に置かない。
- Linux でビルドできない crate（アプリ、`capture-macos`、`capture-windows` の実行）は CI で確かめる。
  `capture-windows` は `cargo check -p capture-windows --target x86_64-pc-windows-msvc --all-targets` で型だけ確かめられる。

## 4. 確かめる

- `AGENTS.md` の完了の定義のコマンドを回し、結果（件数・失敗）を控える。
- 受け入れ条件の「変異」を実際に入れて、テストが落ちることを確かめてから戻す。
- push したら `gh pr checks <PR> --watch` で CI を待つ。赤なら直して push し直す。
  CI の不安定さが原因と思えるときは、再実行する前にログの根拠を PR 本文に書く。

## 5. PR を作る・更新する

- 新しいブランチの最初の push は `git push -u origin <branch>`。PR は `gh pr create`（base は仕様のとおり）。
- PR 本文（更新のときは追記）:

```markdown
## 目的
<仕様へのリンクと 1 文>

## 変更
<ファイル / 挙動ごとに箇条書き。指摘の修正なら「指摘 → 修正」の表>

## 確認
| コマンド / CI | 結果 |
| --- | --- |
| cargo test … | N passed |
| 変異: <内容> | テスト X が落ちる |
| CI（Linux / Windows / macOS） | pass / fail |

## CI でしか確かめられないこと / 実機確認が必要なこと
<無ければ「なし」>

## Open questions
<無ければ「なし」>

## Status
<done / blocked（理由）と次の一歩>
```

- **マージしない**。force-push しない。main の取り込みは `git merge origin/main`。

## 6. 最後のメッセージ

呼び出し元（Claude）が読む。次の順に短く書く: PR の URL、Status、コミットの一覧、
回したコマンドと結果、CI の結果、確かめられなかったこと、Open questions。
