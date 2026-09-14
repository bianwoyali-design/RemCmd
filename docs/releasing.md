# Release Process

RemCmd releases are built by `.github/workflows/release.yml`. A `v*` tag
packages macOS, Windows, and Linux artifacts and creates a GitHub prerelease
when the tag contains a prerelease suffix.

Release metadata validation requires Python 3.11 or newer. CI uses Python 3.12.

## Prepare a Release

1. Create a release branch from the latest `main`.
2. Update the workspace version in `Cargo.toml`.
3. Update the WiX-only numeric version in `packaging/wix/windows.toml`.
4. Refresh `Cargo.lock` and add the release section to `CHANGELOG.md`.
5. Update user-facing channel or platform limitations in `README.md`,
   `docs/installation.md`, and the platform-specific documentation.
6. Run `python3 scripts/release_metadata.py` and the checks required for the
   changed behavior. Reuse successful CI checks for the exact source commit.

For prerelease MSI version mapping, follow
[Windows Code Signing](windows-code-signing.md#msi-versioning).

## Validate Packages Before Tagging

Run the Release workflow manually from the release branch without a
`release_tag` input to produce a release candidate:

```bash
gh workflow run release.yml --ref codex/release-candidate
```

This builds and uploads the macOS DMG, Windows MSI, Linux DEB, and Linux
AppImage, plus a checksum and source-identity manifest, without creating a
GitHub Release. Install and smoke-test the applicable artifacts before merging.

The previous Debian package is versioned `0.1.0-beta.1`, which Debian compares
as newer than the stable `0.1.0` package because the prerelease separator is a
plain hyphen. Treat installation over that legacy package as a documented
downgrade (for example, remove the beta package first or use `apt install
--allow-downgrades`); future stable packages should use a Debian revision or
tilde prerelease scheme if Debian upgrade ordering must remain monotonic.

At minimum, verify:

- the displayed version and package filenames;
- application startup, standard window controls, and the Windows titlebar menus;
- creation of local and SSH terminals;
- password, private-key, passwordless, and SSH Agent authentication where
  supported;
- host-key review and saved credential access;
- SFTP directory listing and one upload/download;
- platform-specific installation warnings documented in the installation
  guide.

## Publish

After the release preparation pull request is merged and publication is
authorized, tag the exact merge
commit on an up-to-date `main`:

```bash
git switch main
git pull --ff-only
git tag -a v0.1.0 -m "RemCmd v0.1.0"
git push origin v0.1.0
```

Do not move or reuse a published tag. The tag push runs the Release workflow,
attaches the generated packages, and marks prerelease versions as GitHub
prereleases.

If packaging succeeds but release creation fails, rerun the workflow manually
with `release_tag` set to the existing tag. This recovery path resolves and builds the exact tagged commit, validates that
the tag and Cargo/MSI versions agree, and creates the release without moving the
tag. Packaging reuses successful CI results for that exact commit, including formatting,
Clippy and the three-platform workspace tests, instead of rerunning the suite in
every packaging job. Wait for CI to complete before starting the Release workflow.
Published releases include `SHA256SUMS` and `BUILD-METADATA.json` for verification.
