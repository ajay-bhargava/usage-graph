# Changelog

## Unreleased

- Abort in-flight account lookup on session shutdown and ignore stale `ctx.ui` after `/new`, `/resume`, `/fork`, or `/reload`, so a late usage-cli result cannot crash Pi.

## 0.1.0

- Add `/codex-account` to pin, auto-rotate, or reauth named Codex subscriptions.
- Swap Pi `openai-codex` OAuth from the `usage` CLI account store.
- Show the live account name in the statusline next to `pi-codex-usage`.
