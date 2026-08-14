# Roadmap

RemCmd is entering public beta. The roadmap below describes the delivered
foundations and the work that remains before a stable release.

## Completed

### Foundation

- Rust workspace, GPUI desktop application, strict CI, and release packaging.

### Connection Management

- Saved SSH profiles, native keychain credentials, private-key file selection,
  and Light, Dark, and System appearance settings.

### SSH and Terminal

- Password, private-key, and passwordless authentication, plus SSH Agent
  authentication on macOS and Linux.
- Host-key verification and explicit first-use fingerprint trust.
- Interactive PTY shell, ANSI rendering, clipboard, selection, scrollback,
  terminal tabs, and nested splits.

### Files and Sessions

- Concurrent SSH sessions, local terminal tabs, multi-server Quick Command,
  and Linux performance monitoring.
- SFTP tree navigation, text editing, recursive transfers, progress,
  cancellation, concurrency, and bandwidth limits.

### Connectivity, Localization, and Diagnostics

- English and Simplified Chinese interfaces with system-language detection,
  English fallback, and immediate runtime switching.
- Read-only OpenSSH configuration import with includes, host and match rules,
  dependency-aware ProxyJump previews, and conflict-safe re-imports.
- HTTP CONNECT, SOCKS5, ProxyCommand, and ordered multi-hop SSH routes with
  independent host-key checks and authentication at every hop.
- Redacted structured diagnostics, seven-day log retention, runtime debug
  filtering, and anonymized ZIP support bundles.

### Distribution

- Automated macOS, Windows, and Linux package artifacts.
- GitHub prereleases for version tags.

## Current Focus

The beta channel focuses on installation validation, compatibility fixes, and
feedback from real SSH and SFTP workloads across macOS, Windows, and Linux.

## Before Stable

- Developer ID signing and notarization for macOS.
- Authenticode signing for Windows.
- Broader installation and upgrade testing across supported platforms.
- Documentation and compatibility improvements driven by beta feedback.

The roadmap is directional rather than a promise of dates or feature order.
