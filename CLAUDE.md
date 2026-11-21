# meetingram-core — Claude 向けガイド

共通の規則は AGENTS.md にある。

@AGENTS.md

## Claude の仕事

meetingram の CLAUDE.md と同じ（設計し、実装は Codex に渡し、戻ってきた差分を読み直し、マージしてタグを打つ）。
skills（`design`、`codex-task`、`review-pr`、`merge-pr`）は meetingram の `.claude/skills/` にあり、
`-R cuzic/meetingram-core` でこの repo も扱う。コアの作業も `~/src/meetingram` で Claude を起動して行う。
