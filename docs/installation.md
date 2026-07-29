# Installation

Download the latest RemCmd package from the
[GitHub Releases page](https://github.com/bianwoyali-design/RemCmd/releases).
Release builds are alpha software. Verify that the release and downloaded file
come from the official `bianwoyali-design/RemCmd` repository before installing.

## macOS

The current macOS build supports Apple silicon and requires macOS 13 or later.

1. Download the `.dmg` file.
2. Follow the [macOS installation notes](macos-installation.md) to open the
   DMG and move `RemCmd.app` to Applications.

The alpha DMG uses an ad-hoc signature and is not notarized. Gatekeeper needs a
one-time confirmation for both the DMG and the app.

## Windows

Download and run the x86_64 `.msi` installer. The current installer is not yet
Authenticode-signed, so Microsoft Defender SmartScreen can require an explicit
confirmation. Check that the installer came from the official release before
choosing **More info** and **Run anyway**.

## Linux

### Debian and Ubuntu

Download the `amd64.deb` package, then install it with:

```bash
sudo apt install ./remcmd_0.1.0-alpha.1_amd64.deb
```

### AppImage

Download the x86_64 AppImage, make it executable, then run it:

```bash
chmod +x remcmd_0.1.0-alpha.1_x86_64.AppImage
./remcmd_0.1.0-alpha.1_x86_64.AppImage
```

RemCmd needs a Wayland or X11 desktop session. A bare WSL terminal has no
compositor and cannot launch the GPUI window; use WSLg or a native Linux desktop
session instead.

## Source Builds

The workspace is pinned to Rust 1.96.1. Build and launch the app with:

```bash
cargo run -p remcmd-app
```

On macOS, install full Xcode for Metal shader compilation. Linux source builds
need the system development packages listed in the release workflow. Run the
workspace checks before opening a pull request:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
