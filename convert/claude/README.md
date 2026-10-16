# membank-claude

Changes Claude Code transcripts into lines that `membank import` reads. Tool calls, thinking and side chains are left out.

```sh
cargo install --path convert/claude
membank-claude ~/.claude/projects/my-project/*.jsonl > chat.jsonl
membank import chat.jsonl
```
