# Changelog

All notable changes to RemCmd are documented in this file.

## [0.1.0-alpha.1] - 2026-07-29

### Added

- Saved SSH connection profiles with Password, Private Key, SSH Agent, and
  Passwordless authentication.
- Native keychain storage for reusable passwords and private-key passphrases.
- Host-key verification through `known_hosts`, including first-use fingerprint
  review and changed-key rejection.
- Interactive SSH terminals with ANSI rendering, selection, clipboard support,
  scrollback, tabs, nested split panes, and per-pane working-directory tracking.
- Embedded local terminal, Quick Terminal, and multi-server Quick Command.
- SFTP tree browsing, editable UTF-8 text files, recursive upload and download,
  transfer progress, cancellation, concurrency, and bandwidth limits.
- Linux server performance monitoring through a dedicated SSH channel.
- Light, Dark, and System themes, terminal font preferences, and configurable
  horizontal or vertical tab layouts.
- macOS DMG, Windows MSI, Linux DEB, and Linux AppImage release artifacts.

### Security

- Secrets remain outside profile JSON and are stored in the native system
  keychain only after successful authentication.
- Unknown SSH host keys require an explicit trust decision before connecting.

### Known Limitations

- macOS alpha builds use an ad-hoc signature and require a Gatekeeper
  first-launch exception.
- Windows alpha installers are not yet Authenticode-signed.
- SFTP features require an SFTP subsystem on the remote server.
