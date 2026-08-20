# usage ⚡

See remaining **Codex** and **xAI** subscription quota — multi-account, one static print.

No session scanning. No watch loop. Login explicitly, run `usage`, done.

## Install

```bash
cargo install --git https://github.com/ajay-bhargava/usage-graph --locked
```

## Quick start

```bash
usage login --provider codex --name work
usage login --provider xai --name grok
usage
```

## Output

```text
Subscription Remaining

work  (codex, pro)
+------------------------------------------------+
| 5h     [############--------]  58% left, 3d 2h |
| Weekly [##################--]  91% left, 45s   |
+------------------------------------------------+

grok  (xai, SuperGrok)
+------------------------------------------------+
| Weekly [############--------]  67% left, 5d 4h |
+------------------------------------------------+
```

UTF-8 terminals get box-drawing + `█`/`░` bars. Piped output stays ASCII.

## Commands

| Command | What it does |
| --- | --- |
| `usage` | Print remaining quota for every account |
| `usage --json` | Same data as JSON |
| `usage --account work` | One account only |
| `usage login --provider codex --name work` | Codex device login |
| `usage login --provider xai --name grok` | xAI device login |
| `usage login … --import --auth-file PATH` | Import Codex / Pi / Grok auth |
| `usage list` | List names (no secrets) |
| `usage logout work` | Remove an account |

## Storage

```text
~/.config/usage-cli/accounts.json   # 0600
```

Never auto-imports. Never writes back to `~/.codex`, Pi, or `~/.grok`.

## Notes

- Codex → ChatGPT `wham/usage` (incl. Spark when present)
- xAI → Grok CLI proxy credits (`Weekly` / `Monthly` / `Credits`)
- API keys are not subscription auth — they won't work here
