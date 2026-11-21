# meetingram-core — エージェント共通ガイド（Codex・Claude 共通）

会議の録音・文字起こし・イベント配信・Rhai プラグイン・要約の Rust crate 群。
アプリ [meetingram](https://github.com/cuzic/meetingram)（`~/src/meetingram`）がタグ固定で使う。
設計は `docs/adr/`（0015 以降。0001〜0014 は 1on1-recorder から引き継いだリスク台帳で形式が違う）と、
`design.md`、`stt-transcription-architecture.md`、`docs/*.md` にある。実装の前に担当の ADR を読むこと。

## 役割分担

meetingram と同じ。Claude が設計・タスク仕様・レビュー・マージとタグ付け、Codex が実装・指摘の修正・
main の取り込み、CI が Windows と macOS での最終判定、owner が仕様の判断と実機確認。
仕事は `[codex] …` の Issue か、PR の `## [codex] タスク仕様` コメントで渡す。仕様に無い判断は
PR 本文の「Open questions」に書いて止める。Codex の手順は `.agents/skills/implement-task/`、
Claude の手順（skills）は meetingram の `.claude/skills/` にある。

## 完了の定義

```sh
cargo test --workspace --exclude capture-macos --exclude capture-windows --exclude stt-whisper
cargo check -p capture-windows --target x86_64-pc-windows-msvc --all-targets   # Linux から Windows 向けの型チェック
gh pr checks <PR> --watch                                                       # Windows と macOS の build（clippy・テスト）
```

- 3 つとも通ること。`capture-macos` と `app-service` の OS 別機能（`windows-supervisor`、`macos-supervisor`、
  `live-transcription`）は CI でしか確かめられない。CI を通す前に「動く」と報告しない。
- `e2e-best-effort` ジョブ（仮想オーディオでのキャプチャのスモークテスト）は失敗を許すが、落ちたらログを読む。
- 触った crate で `cargo clippy -p <crate> --all-targets --no-deps` の警告を増やさない
  （全体の clippy と rustfmt はまだ整備していない。触っていないファイルを整形しない）。
- 新しい挙動にはテストを付け、そのテストが落ちる変更（手書きの変異でよい）を 1 つ確かめる。
- 公開 API・イベント・設定・保存形式を変えたら、PR 本文に書く（アプリの追従とタグの番号に効く）。

## 知らないと壊すこと

- `stt-google` のビルドに `protoc` が要る。`stt-whisper` は whisper.cpp をビルドするので既定の対象外。
- `rhai-engine` の `jsonwebtoken` は `aws_lc_rs` 機能が必須（外すと最初の署名で panic。
  `rag::vertex::tests::token_request_jwt_is_signed_with_rs256` が守っている）。
- `session-store` の `rusqlite` は `fallible_uint` 機能で `u64` を読み書きしている。
- `credential-store` は `keyring` の `v1` API を使い、OS のキーリングが無いときは暗号化ファイルに切り替える。
  切り替えの判定はエラー文の照合（`should_fallback`）なので、`keyring` を上げたら文言を確かめる。
- キーリングのサービス名、データディレクトリ、制御ソケットの名前は `meetingram`。変えると既存の設定と録音が見えなくなる。
- `rhai` は `=` で固定し、アプリと同じ版にする。
- Rhai の hook から呼んだヘルパー関数はモジュールの変数を読めない（`plugins/default/hint.rhai` 冒頭の注記）。
- macOS のマイク録音は macOS 15 以上でしか有効にできない（`capture-macos/src/sc_stream.rs`）。

## 安全規則

- ログ・メトリクス・診断出力に、API キー、文字起こしや要約の本文、生のデバイス ID を出さない（`docs/logging-policy.md`）。
- 実在の会議の音声やテキストを fixture にしない。テストには合成データを使う。
- プローブや一時ファイルは repo の外に置く。コミットやコメントにマシン固有の情報を入れない。
- 人の承認が要る操作: 新しい外部サービスへの送信（STT・LLM・RAG のプロバイダ追加）、ライセンスの変更。

## Git とリリース

- ブランチ名、merge で追従（rebase / force-push しない）、Codex はマージしない、`Co-Authored-By:`、
  指摘 ID の書き方は meetingram の AGENTS.md と同じ。
- アプリに届ける変更は、マージ後に Claude がタグを打つ（0.x の間は、公開 API・イベント・設定・保存形式が
  変わるなら minor、それ以外は patch）。タグは消したり付け替えたりしない。

## repo の地図

| 場所 | 中身 |
| --- | --- |
| `crates/capture-*`, `audio-timeline` | OS ごとの音声キャプチャ（WASAPI / ScreenCaptureKit）と、トラックの時刻合わせ |
| `crates/recorder-domain`, `segment-store`, `session-store`, `upload-client` | 録音の型、Opus セグメントの書き込み、SQLite のセッション・文字起こし・要約、アップロード |
| `crates/stt-*` | 文字起こし。`stt-api` が共通の型、各社のアダプタが `SttEvent` を返す |
| `crates/transcript-event`, `local-broker` | `SttEvent` → `TranscriptEvent` の変換と、subject 単位の pub/sub（`docs/decouple-summary-transcription.md`） |
| `crates/rhai-engine`, `plugins/default/` | Rhai プラグインの実行と標準プラグイン（`docs/rhai-plugin-summary.md`） |
| `crates/summarize` | `genai` による要約と、CLI（claude / codex）経由の要約 |
| `crates/app-service`, `control-protocol`, `credential-store` | 録音セッションの組み立て、制御ソケット、キーの保存 |
| `docs/adr/`, `docs/reviews/` | 設計と PR のレビュー記録 |
