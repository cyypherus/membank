# membank-codex

Changes Codex session logs into lines that `membank import` reads. Only completed user and agent messages are kept.

```sh
cargo install --path convert/codex
membank-codex ~/.codex/sessions/2026/10/*/*.jsonl > chat.jsonl
membank import chat.jsonl
```
