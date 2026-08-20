# usage

Print remaining ChatGPT Codex subscription quota for accounts you explicitly log in.

This is not a session-log cost scanner. It does not read `~/.codex` or Pi auth unless you pass `--import --auth-file`.

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

# Import from an explicit Codex CLI or Pi auth file
usage login --provider codex --name personal --import --auth-file /path/to/auth.json

usage list
usage logout work

# Remaining quota for every logged-in account
usage
usage --json
usage --account work
usage --offline
```

xAI SuperGrok login is not implemented yet.

## Sample

```text
Subscription Remaining

work  (codex, pro)
+--------------------------------------------------+
| 5h     [############--------]  58% left, 3d 2h   |
| Weekly [##################--]  91% left, 45s     |
+--------------------------------------------------+
```

`--json` emits `accounts[].windows[]` with `used_percent`, `left_percent`, and `reset_in`. Per-account fetch failures become `error` instead of aborting the run.

## Storage

Credentials live in:

```text
${XDG_CONFIG_HOME:-~/.config}/usage-cli/accounts.json
```

The directory is `0700` and the file is `0600`. Override the path with `--store`. Tokens are never written back to Codex CLI or Pi.

## Notes

Quota data comes from the unofficial ChatGPT `wham/usage` endpoint used by Codex CLI. OpenAI API keys cannot read these windows. Spark windows are shown when the payload includes them.
