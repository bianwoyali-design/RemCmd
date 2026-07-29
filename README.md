# RemCmd

RemCmd is a GPU-accelerated SSH terminal and SFTP client built with Rust and
GPUI. It combines saved SSH connections, interactive terminal sessions, split
panes, remote file management, and local terminals in one desktop application.

RemCmd is currently in alpha. Download the latest build from the
[GitHub Releases page](https://github.com/bianwoyali-design/RemCmd/releases).

## Highlights

- SSH Password, Private Key, Passwordless, and SSH Agent authentication.
- Host-key verification backed by `~/.ssh/known_hosts`, including explicit
  first-use fingerprint review.
- Interactive ANSI terminal with scrollback, selection, clipboard support,
  tabs, and nested split panes.
- Local terminal tabs and a Quick Command panel that sends one command to
  selected connected servers.
- SFTP browser with tree navigation, multi-selection, recursive transfer,
  editable text files, conflict checks, progress, cancellation, concurrency,
  and bandwidth limits.
- Per-server Linux performance monitoring for CPU, memory, swap, disk I/O,
  process counts, and sampling latency.
- Light, Dark, and System themes, configurable terminal font settings, and
  horizontal or vertical terminal tabs.

## Install

Release assets are currently available for:

- macOS on Apple silicon (`.dmg`)
- Windows x86_64 (`.msi`)
- Linux x86_64 (`.deb` and `.AppImage`)

See the [installation guide](docs/installation.md) for platform-specific
steps and alpha limitations. macOS users should also read the
[macOS installation notes](docs/macos-installation.md).

## First Connection

1. Open **New Connection** and enter the server host, port, and user.
2. Choose Password, Private Key, SSH Agent, or Passwordless authentication.
3. Connect and verify an unknown host-key fingerprint before trusting it.
4. Open **Remote Files** after connection when the server offers an SFTP
   subsystem.

Reusable passwords and private-key passphrases are saved only after successful
authentication in the operating system keychain. They are never written to the
profile JSON file.

## Build From Source

RemCmd requires Rust 1.96.1. On macOS, install full Xcode because GPUI builds
Metal shaders. Then run:

```bash
cargo run -p remcmd-app
```

For the workspace checks used by CI:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Documentation

- [Installation](docs/installation.md)
- [macOS alpha installation](docs/macos-installation.md)
- [Windows packaging and future signing](docs/windows-code-signing.md)
- [Roadmap](ROADMAP.md)
- [Changelog](CHANGELOG.md)

## License

RemCmd is licensed under [Apache-2.0](LICENSE).
