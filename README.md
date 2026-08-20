# usage

Print remaining Codex and xAI SuperGrok subscription quota for accounts you explicitly log in.

This is not a session-log cost scanner. It does not read `~/.codex`, Pi, or `~/.grok` auth unless you pass `--import --auth-file`.

## Install

```bash
cargo install --git https://github.com/ajay-bhargava/usage-graph --locked
```

From a checkout:

```bash
cargo install --path . --locked
```

## Commands

```bash
# Device-code login (required before any report)
usage login --provider codex --name work
usage login --provider xai --name grok

# Import from an explicit auth file
usage login --provider codex --name personal --import --auth-file /path/to/auth.json
usage login --provider xai --name grok --import --auth-file ~/.grok/auth.json

usage list
usage logout work

# Remaining quota for every logged-in account
usage
usage --json
usage --account work
usage --offline
```

xAI import accepts Grok CLI `auth.json` (OIDC `key` entries) or Pi `xai` OAuth. API keys are rejected.

## Sample

```text
Subscription Remaining

work  (codex, pro)
+--------------------------------------------------+
| 5h     [############--------]  58% left, 3d 2h   |
| Weekly [##################--]  91% left, 45s     |
+--------------------------------------------------+

grok  (xai, SuperGrok)
+------------------------------------------------+
| Weekly [############--------]  67% left, 5d 4h |
+------------------------------------------------+
```

`--json` emits `accounts[].windows[]` with `used_percent`, `left_percent`, and `reset_in`. Per-account fetch failures become `error` instead of aborting the run.

## Storage

Credentials live in:

```text
${XDG_CONFIG_HOME:-~/.config}/usage-cli/accounts.json
```

The directory is `0700` and the file is `0600`. Override the path with `--store`. Tokens are never written back to Codex CLI, Pi, or Grok.

## Notes

Codex quota comes from the unofficial ChatGPT `wham/usage` endpoint. Spark windows are shown when the payload includes them.

xAI quota comes from the unofficial Grok CLI proxy `billing?format=credits` endpoint. The row is labeled Weekly, Monthly, or Credits from the reported period length. OpenAI and xAI API keys cannot read these subscription windows.
