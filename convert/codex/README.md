# membank-codex

Changes Codex session logs into lines that `membank import` reads. Only completed user and agent messages are kept.

```sh
cargo install --path convert/codex
membank-codex ~/.codex/sessions/2026/10/*/*.jsonl > chat.jsonl
membank import chat.jsonl
```

To keep a bank current, add these hooks to `~/.codex/hooks.json`. `SessionStart` gives each new session the context. `Stop` runs after each turn: `membank-codex` reads the transcript path from the hook's input, the session is imported, and compaction runs in the background. Codex needs JSON from a Stop hook, so it ends with `echo '{}'`.

```json
{
  "hooks": {
    "SessionStart": [{ "hooks": [{ "type": "command", "command": "membank context --budget 7500" }] }],
    "Stop": [{ "hooks": [{ "type": "command", "command": "membank-codex | membank import /dev/stdin && (membank compact -- codex exec - >/dev/null 2>&1 &); echo '{}'" }] }]
  }
}
```
