# Apple Silicon macOS installation

Download assets from https://github.com/ajay-bhargava/usage-graph/releases.
These binaries are for Apple Silicon (M-series) Macs, not Intel Macs or Linux.
They are not Apple-signed or notarized. Only run downloads you trust; macOS may
require approval under System Settings > Privacy & Security.

## Complete Pi bundle (no npm, Git, or Rust required)

Download `pi-codex-bundle-macos-arm64.zip` and extract it into a permanent location,
for example `~/pi-codex-bundle-macos-arm64`. With Pi already installed:

```sh
mkdir -p ~/.local/bin
install -m 755 ~/pi-codex-bundle-macos-arm64/bin/usage ~/.local/bin/usage
export PATH="$HOME/.local/bin:$PATH"
pi install ~/pi-codex-bundle-macos-arm64
usage login --provider codex --name personal
usage login --provider codex --name work
```

Add `export PATH="$HOME/.local/bin:$PATH"` to `~/.zshrc` to persist it.
Restart Pi from a shell with that PATH. Use `/codex-account` to choose an account,
`/codex-account auto` for automatic selection, and `/codex-account status` to inspect it.
The quota bar appears when using an OpenAI Codex subscription model.

Pi loads the extensions directly from the extracted directory; do not delete or move it.
Remove/disable separately installed copies of either extension before loading this bundle.
The account switcher intentionally updates Pi's `openai-codex` credential in its auth store.
Never share your Pi auth file or the usage CLI's account store with another user.

## CLI only

Download `usage-macos-arm64.tar.gz`, then:

```sh
tar -xzf usage-macos-arm64.tar.gz
mkdir -p ~/.local/bin
install -m 755 usage-macos-arm64/bin/usage ~/.local/bin/usage
export PATH="$HOME/.local/bin:$PATH"
usage login --provider codex --name personal
usage
```

## Verify downloads

Download `SHA256SUMS` alongside both archives and run `shasum -a 256 -c SHA256SUMS`.
If downloading only one archive, verify its matching line from `SHA256SUMS`.
Checksums detect corruption; they are not Apple code signing.

## Source installation

With Rust 1.88+ and a working native build toolchain (on macOS, Xcode Command Line Tools):

```sh
cargo install --git https://github.com/ajay-bhargava/usage-graph --locked
pi install git:github.com/ajay-bhargava/usage-graph
```

Pi installation only installs the extensions, not the CLI. A downloaded source ZIP
can instead be installed with `cargo install --path /path/to/usage-graph --locked`
and `pi install /path/to/usage-graph`. Cargo downloads Rust dependencies.
