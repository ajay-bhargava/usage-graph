# pi-claude-usage

Claude Pro/Max remaining-quota status bar for Pi. Shown only while the active model's
provider is `claude-code` (earendil-works/pi-claude-code) or `anthropic`.

```text
claude ██████████ 6.7d        top half = 5h window, bottom half = weekly
claude ▄▄▄▄▄▄▄▄▄▄ 2h/2d       5h exhausted: 5h reset / weekly reset
```

`/claude-usage` prints every window (5h, Weekly, per-model weekly limits).

## Sources

1. `usage --json` for a `claude` account (`usage login --provider claude --name max`).
   Pin one with `PI_CLAUDE_USAGE_ACCOUNT=max`.
2. Fallback: Claude Code's `~/.claude/.credentials.json` (or `$CLAUDE_CONFIG_DIR`),
   read-only. Claude Code keeps the token fresh; this extension never refreshes or writes it.

Refreshes on session start, model switch, after each agent run (at most every 30s),
and every 2 minutes.
