# Project Purpose

`usage` is a static CLI for remaining Codex (and later xAI) subscription quota across explicitly logged-in accounts. It does not scan session logs.

# Engineering Rules

- Keep login explicit. Never auto-import `~/.codex/auth.json` or `~/.pi/agent/auth.json`.
- Store credentials only under the usage-cli config path, mode `0600`. Never write back to Codex CLI or Pi auth files.
- Treat `chatgpt.com/backend-api/wham/usage` as an unofficial contract: fixture-test payload variants and degrade to `unavailable`.
- Do not implement xAI in this phase.
- Enforce crate-level `clippy::pedantic`, `clippy::unwrap_used`, `missing_docs`, and `rustdoc::missing_crate_level_docs`.
- Characterize parsing, store, and render behavior with fixture tests before changing quota semantics.
