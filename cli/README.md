# membank

Builds and reads a bank: one chat stored as a tree of summaries.

```sh
cargo install --path cli
```

Commands work on the bank at `~/.membank/global`. Use `--bank PATH` or set `MEMBANK` to use another one.

```sh
membank import chat.jsonl                    # add messages; a directory imports every .jsonl in it
membank compact --jobs 8 -- llm -m my-model  # summarize new messages with any command
membank context --budget 32000               # print the context, cut to a byte budget
membank zoom 512 256                         # open line 512+256 (messages 512 to 767) into its two halves
membank zoom 7 1                             # print message 7 in full
membank date 7                               # print when message 7 was sent
membank search deploy fix                    # find lines and messages that match these words (typos allowed)
membank prune 7 9                            # clear messages 7 and 9 and the summaries built from them
```

An import file has one JSON object per line. `kind` is `user` or `assistant`. `src` is a unique id, so importing a message twice adds it once. `session` names the conversation the message belongs to.

Import puts each session at its start date. An older session goes between the messages around that date and stays in one piece; summaries after it are kept, and only the ones it splits are made again. Use [membank-claude](../convert/claude/README.md) or [membank-codex](../convert/codex/README.md) to make one from Claude Code or Codex transcripts.

```json
{"kind":"user","text":"hello","date":"2026-10-15T01:00:00Z","src":"unique-id","session":"chat-1"}
```

`compact` writes each prompt to the command's stdin and reads one summary line from its stdout. `--system FILE` sends FILE as the system prompt instead of the built-in one.
