# meetingram-core

会議の録音・文字起こし・イベント配信のための Rust crate 群です。
[meetingram](https://github.com/cuzic/meetingram)(会議を録音しながら議論を図にするツール)のコアとして使います。

Rust crates for capturing meeting audio, live transcription, and an event broker with Rhai plugins. This is the core of [meetingram](https://github.com/cuzic/meetingram).

[1on1-recorder](https://github.com/cuzic/1on1-recorder) の `crates/` と `plugins/` を引き継いで始めました。設計ドキュメント(`design.md` など)と ADR も 1on1-recorder 時代のものです。

## crate の構成

| 分類 | crate |
|------|-------|
| 音声キャプチャ | `capture-api`、`capture-windows`、`capture-macos`、`audio-timeline` |
| 録音データ | `recorder-domain`、`segment-store`、`session-store`、`upload-client` |
| 文字起こし | `stt-api`、`stt-deepgram`、`stt-openai`、`stt-google`、`stt-assemblyai`、`stt-whisper` |
| イベントと後段処理 | `transcript-event`、`local-broker`、`rhai-engine`、`summarize` |
| アプリの土台 | `app-service`、`control-protocol`、`credential-store` |

`plugins/default/` は Rhai の標準プラグイン(`std.rhai`、`summary.rhai`、`hint.rhai`)です。`rhai-engine` のテストもこれを読み込みます。

## アプリから使う

git 依存で参照します。

```toml
[dependencies]
app-service = { git = "https://github.com/cuzic/meetingram-core", tag = "v0.1.0" }
```

キーリングのサービス名、データディレクトリ、制御ソケットの名前は `meetingram` です。1on1-recorder とは別の名前なので、同じ PC に両方を入れても設定と録音データは混ざりません。

## ビルド

- `capture-macos` は macOS、`capture-windows` は Windows でしかビルドできません。Linux では `cargo test --workspace --exclude capture-macos --exclude capture-windows --exclude stt-whisper` を使います。
- `stt-whisper` は whisper.cpp をビルドするため、既定のビルド対象から外しています。必要なときは `cargo build -p stt-whisper` で明示します。
- `stt-google` のビルドには `protoc` が必要です。

## ライセンス

MIT または Apache-2.0 のどちらかを選べます。
