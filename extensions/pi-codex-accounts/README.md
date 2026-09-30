# pi-codex-accounts

Rotate and pin named ChatGPT Codex subscriptions inside Pi. Quota comes from the `usage` CLI. Pi keeps one live `openai-codex` OAuth slot, so this extension copies the chosen account into `~/.pi/agent/auth.json`. `@llblab/pi-codex-usage` then shows that account's weekly bar.

The repository's root Pi manifest loads this extension **before** the bundled
`pi-codex-usage`. Do not also load separately installed copies.

## Install

```sh
pi install git:github.com/ajay-bhargava/usage-graph
```

Or run `pi install /absolute/path/to/usage-graph` for an extracted local bundle.
This installs both extensions, not the CLI. Restart Pi or `/reload`.

Requires `usage` on `PATH` with Codex accounts already logged in:

```sh
usage login --provider codex --name work
usage login --provider codex --name personal
usage login --provider codex --name consulting
usage login --provider codex --name amp
```

## Commands

| Command | Result |
| --- | --- |
| `/codex-account` | Selector: auto or a named account |
| `/codex-account auto` | Unpin and pick the highest remaining weekly window |
| `/codex-account amp` | Pin `amp` and swap Pi auth |
| `/codex-account status` | Notify current mode and remaining |
| `/codex-account reauth amp` | Device-code dialog, then pin `amp` |

Auto mode picks the Codex account with the most weekly remaining. That equalizes drawdown over the week. Pin mode stays on that account even at 0%.

Unauthorized pins open the same device-code dialog as Pi `/login`. Esc cancels and leaves the previous account in place. Revive a dead name from the CLI with `usage reauth amp` if you are not in the TUI.

## Statusline

```
amp · auto       codex 98% 6d 23h
```

The left chip is this extension. The right chip is `pi-codex-usage`.
