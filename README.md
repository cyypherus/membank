# membank

membank keeps one long chat as a memory an assistant can read. It compresses the chat into a binary tree of short summaries.

The **context** is the text you give the assistant at the start of each call, so it remembers the chat. It is the whole chat as a list of one-line summaries, oldest first, kept under a fixed size. Recent messages get one line each, and older messages are merged into fewer lines. Each line starts with `id+n|`, which means it summarizes the `n` messages that start at message `id`. The assistant can zoom into any line to get the detail back.

- [membank](cli/README.md): the command line tool that builds and reads a bank
- [membank-claude](convert/claude/README.md), [membank-codex](convert/codex/README.md): turn a harness's transcripts into membank's import format; `membank-convert` is the small library they share
- [membank-gui](gui/README.md): a viewer for a bank
