# Changelog

All notable changes to RemCmd are documented in this file.

## [Unreleased]

## [0.1.0-beta.1] - 2026-07-30

### Added

- A RemCmd-owned vector wordmark, application About window, and About entries
  in the sidebar and native application menu.
- A Windows-specific glass titlebar with the RemCmd icon and identity, standard
  minimize, maximize, and close hit targets, a persistent connection search
  field, and integrated functional File, Edit, Terminal, View, Window, and Help
  menus.

### Changed

- Terminal rendering now reuses unchanged snapshots, updates only damaged
  rows, and coalesces redraw notifications to reduce CPU and GPU work.
- SSH startup now races resolved addresses, enables `TCP_NODELAY`, batches
  ready events, and keeps shell detection outside the visible terminal.
- Shell working-directory integration installs after startup output settles,
  preserving remote login banners and prompt engines while keeping integration
  commands hidden.
- Sidebar and titlebar transitions now share motion timing, and long
  single-select menus virtualize their rows for smoother scrolling.

### Fixed

- Manual tab scrolling remains responsive while new-tab scroll animations run.
- Opposite sidebar transitions no longer reposition the left sidebar control.
- Single-select menus open near the selected option and retain full-width,
  theme-correct hover rows.
- Home connection rows preserve their rounded hover corners.
- Windows limits Mica to the draggable window titlebar; the menu and terminal
  tab rows use opaque application surfaces with explicit menu text colors.

### Known Limitations

- macOS builds use an ad-hoc signature and require a Gatekeeper first-launch
  exception.
- Windows installers are not yet Authenticode-signed.
- The Windows glass titlebar and menu placement require broader validation
  across Windows versions and display scaling configurations.
- SSH Agent authentication currently requires a Unix `SSH_AUTH_SOCK` and is
  unavailable on Windows.
- SFTP features require an SFTP subsystem on the remote server.

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

[Unreleased]: https://github.com/bianwoyali-design/RemCmd/compare/v0.1.0-beta.1...HEAD
[0.1.0-beta.1]: https://github.com/bianwoyali-design/RemCmd/compare/v0.1.0-alpha.1...v0.1.0-beta.1
[0.1.0-alpha.1]: https://github.com/bianwoyali-design/RemCmd/releases/tag/v0.1.0-alpha.1
