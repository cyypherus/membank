# membank-claude

Changes Claude Code transcripts into lines that `membank import` reads. Tool calls, thinking and side chains are left out.

```sh
cargo install --path convert/claude
membank-claude ~/.claude/projects/my-project/*.jsonl > chat.jsonl
membank import chat.jsonl
```

To keep a bank current, add these hooks to `~/.claude/settings.json`. `SessionStart` gives each new session the context. `Stop` runs in the background after each reply: `membank-claude` reads the transcript path from the hook's input, then the session is imported and compacted.

```json
"hooks": {
  "SessionStart": [{ "hooks": [{ "type": "command", "command": "membank context --budget 7500" }] }],
  "Stop": [{ "hooks": [{ "type": "command", "async": true, "command": "membank-claude | membank import /dev/stdin && membank compact -- claude -p --model claude-haiku-5-5 --tools ''" }] }]
}
```
