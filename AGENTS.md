# RemCmd Agent Notes

This file is a navigation guide, not a project history. Check `git status`,
`git log`, pull requests, and release pages for current state.

## Where To Look

- Public overview and setup: `README.md`
- User-facing release history: `CHANGELOG.md`
- Public product direction: `ROADMAP.md`
- Platform installation and signing notes: `docs/`
- CI checks and platform test matrix: `.github/workflows/ci.yml`
- Release workflow: `.github/workflows/release.yml`
- Generated Release Notes categories: `.github/release.yml`
- Issue forms: `.github/ISSUE_TEMPLATE/`
- Workspace version and shared dependencies: `Cargo.toml`
- Windows MSI version override: `packaging/wix/windows.toml`

## Source Map

- GPUI application and views: `crates/remcmd-app`
- Shared domain types and PTY sizing: `crates/remcmd-core`
- Local PTY worker: `crates/remcmd-local`
- Profile and settings persistence: `crates/remcmd-storage`
- SSH transport, host keys, sessions, and SFTP/SCP: `crates/remcmd-ssh`
- Terminal parser and screen state: `crates/remcmd-terminal`
- Structured diagnostics and support bundles: `crates/remcmd-diagnostics`
- Release discovery and verified downloads: `crates/remcmd-update`

Shared implementation entry points:

- Remote-file commands and protocol routing: `crates/remcmd-ssh/src/remote_files.rs`
- Transfer execution and planning: `crates/remcmd-ssh/src/transfer.rs` and
  `crates/remcmd-ssh/src/transfer/plan.rs`
- Text editing shared by input fields and the file editor:
  `crates/remcmd-app/src/text_edit.rs`

## Change Boundaries

- Keep UI code in `remcmd-app`; keep reusable terminal, SSH, storage, and
  domain behavior in their owning crates.
- Reuse shared implementations above when extending the same behavior.
- Never persist real passwords or private-key passphrases in JSON, logs,
  tests, screenshots, or documentation. Reusable secrets belong in the system
  keychain only. Synthetic test fixtures are allowed and must never reuse
  real credentials.
- Preserve user changes in a dirty worktree. Do not reset or revert unrelated
  files.

## UI and Validation

- Keep the UI close to a native macOS application while preserving the
  conventions and functionality of each supported platform.
- Choose local checks for the changed behavior and owning crates. The commands
  below are the complete integration suite, not a requirement to rerun the
  entire suite after every edit, commit, PR update, or packaging step.
- Retain required CI and reuse results applicable to the current commit,
  configuration, and platform. Repeat or broaden checks when changes,
  failures, or missing coverage justify it.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Use `cargo run -p remcmd-app` when the changed behavior requires desktop
validation. Consolidate full visual and installation acceptance on an
integrated candidate. Packaging and release artifacts are built through the
Release workflow; a successful source build alone does not prove installer
or upgrade behavior.

When the user requests separate PRs, group changes by independently reviewable
behavior. Follow the session's agreed merge/release scope and repository
protections. A release-ready result includes verifiable candidate artifacts
and an accurate account of any untested platform or external blocker.

## Code Review Rules

- Perform a basic correctness and regression review of the diff and directly
  related code. Report only concrete defects introduced or worsened by the
  change in supported usage, such as broken connections, terminal input,
  file transfers, data loss, or credential exposure. Respect intentional
  behavior changes.
- Skip style and naming preferences, optional refactors, hypothetical failure
  chains, and speculative hardening. Do not request extra guards, fallbacks,
  retries, abstractions, or tests unless they address a demonstrated defect
  or an explicit project requirement.
- Keep each finding concise: identify the affected code, triggering condition,
  user impact, and smallest useful correction. Leave formatting and lint to
  CI, and keep validation proportional to the change. If no concrete defect
  is found, report no findings.
