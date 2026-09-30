# Project Purpose

`usage` is a static CLI for remaining Codex and xAI subscription quota across explicitly logged-in accounts. It does not scan session logs.

# Engineering Rules

- Keep login explicit. Never auto-import `~/.codex/auth.json`, `~/.pi/agent/auth.json`, or `~/.grok/auth.json`.
- The standalone Rust CLI stores credentials only under the usage-cli config path, mode `0600`. Never write back to Codex CLI, Pi, or Grok auth files from the CLI.
- The separately installed `extensions/pi-codex-accounts` Pi integration intentionally copies the selected account into Pi's auth store. Keep this behavior confined to that opt-in extension; never bundle credentials or personal account state.
- Treat `chatgpt.com/backend-api/wham/usage` and `cli-chat-proxy.grok.com/v1/billing` as unofficial contracts: fixture-test payload variants and degrade to `unavailable`.
- Enforce crate-level `clippy::pedantic`, `clippy::unwrap_used`, `missing_docs`, and `rustdoc::missing_crate_level_docs`.
- Characterize parsing, store, and render behavior with fixture tests before changing quota semantics.
