# Release Process

RemCmd releases are built by `.github/workflows/release.yml`. A `v*` tag
packages macOS, Windows, and Linux artifacts and creates a GitHub prerelease
when the tag contains a prerelease suffix.

## Prepare a Release

1. Create a release branch from the latest `main`.
2. Update the workspace version in `Cargo.toml`.
3. Update the WiX-only numeric version in `packaging/wix/windows.toml`.
4. Refresh `Cargo.lock` and add the release section to `CHANGELOG.md`.
5. Update user-facing channel or platform limitations in `README.md`,
   `docs/installation.md`, and the platform-specific documentation.
6. Run the complete validation suite.

For prerelease MSI version mapping, follow
[Windows Code Signing](windows-code-signing.md#msi-versioning).

## Validate Packages Before Tagging

Run the Release workflow manually from the release branch without a
`release_tag` input:

```bash
gh workflow run release.yml --ref release/v0.1.0-beta.1
```

This builds and uploads the macOS DMG, Windows MSI, Linux DEB, and Linux
AppImage as workflow artifacts without creating a GitHub Release. Install and
smoke-test the applicable artifacts before merging the release branch.

At minimum, verify:

- the displayed version and package filenames;
- application startup and standard window controls;
- creation of local and SSH terminals;
- password, private-key, passwordless, and SSH Agent authentication where
  supported;
- host-key review and saved credential access;
- SFTP directory listing and one upload/download;
- platform-specific installation warnings documented in the installation
  guide.

## Publish

After the release preparation pull request is merged, tag the exact merge
commit on an up-to-date `main`:

```bash
git switch main
git pull --ff-only
git tag -a v0.1.0-beta.1 -m "RemCmd v0.1.0-beta.1"
git push origin v0.1.0-beta.1
```

Do not move or reuse a published tag. The tag push runs the Release workflow,
attaches the generated packages, and marks prerelease versions as GitHub
prereleases.

If packaging succeeds but release creation fails, rerun the workflow manually
with `release_tag` set to the existing tag. This recovery path rebuilds the
artifacts and creates the release without moving the tag.
