# usage ⚡

See remaining **Codex** and **xAI** subscription quota — multi-account, one static print.

No session scanning. No watch loop. Login explicitly, run `usage`, done.

## Install

```bash
cargo install --git https://github.com/ajay-bhargava/usage-graph --locked
```

Apple Silicon Mac users can download a CLI binary or a complete Pi bundle from
[GitHub Releases](https://github.com/ajay-bhargava/usage-graph/releases).
See [installation instructions](INSTALL.md) for the no-npm/no-Git path.

## Pi extensions

Install both companion extensions (account switcher first, quota bar second):

```sh
pi install git:github.com/ajay-bhargava/usage-graph
```

This installs **only the extensions**. Install `usage` separately and ensure it is
on Pi's `PATH`, then log in with `usage login --provider codex --name personal`.
Restart Pi and run `/codex-account`. Disable any separately installed copies of
these extensions to avoid duplicate commands and status bars.

- [pi-codex-accounts](extensions/pi-codex-accounts/README.md): pin or automatically
  select named Codex subscriptions. Unlike the standalone CLI, this opt-in extension
  intentionally copies the selected credential into Pi's auth store.
- [pi-codex-usage](extensions/pi-codex-usage/README.md): remaining quota status bar,
  vendored from [`@llblab/pi-codex-usage` v0.10.1](https://github.com/llblab/pi-codex-usage),
  a fork of narumiruna's extension. Original MIT license and attribution are retained
  in its directory.

The account extension is copied from `@pi-kaush/pi-codex-accounts` v0.1.0 with its
MIT license retained. No credentials or personal account state are bundled.

## Quick start

```bash
usage login --provider codex --name work
usage login --provider xai --name grok
usage
```

## Output

```text
Subscription Remaining

work  (codex, work@example.com, pro)
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
| `usage login … --replace` | Overwrite an existing name |
| `usage reauth work` | Repeat device login for an existing name |
| `usage reauth work --json` | Same, with NDJSON `device_code` / `complete` events |
| `usage list` | List names (no secrets) |
| `usage logout work` | Remove an account |

## Storage

```text
# Linux (or $XDG_CONFIG_HOME/usage-cli/accounts.json)
~/.config/usage-cli/accounts.json
# macOS
~/Library/Application Support/usage-cli/accounts.json
# Credential files use mode 0600.
```

Never auto-imports. Never writes back to `~/.codex`, Pi, or `~/.grok`.

## Notes

- Codex → ChatGPT `wham/usage` (incl. Spark when present)
- xAI → Grok CLI proxy credits (`Weekly` / `Monthly` / `Credits`)
- API keys are not subscription auth — they won't work here
