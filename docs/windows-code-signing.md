# Windows Code Signing

Release tags use WiX to create an MSI package. A manual workflow run publishes
an unsigned MSI for installation testing. A `v*` tag must be signed by
SignPath; the workflow refuses to publish an unsigned Windows release.

## MSI Versioning

WiX requires a numeric `major.minor.patch.build` version, whereas the
user-facing Cargo version may use SemVer prerelease labels. The WiX-only
`packaging/wix/Packager.toml` therefore owns the MSI version.

For `v0.1.0-alpha.1`, the Cargo version remains `0.1.0-alpha.1`, while the
MSI version is `0.0.0.1`. This keeps the alpha installer older than the future
`0.1.0` final installer. Before publishing another prerelease, update the
Windows version monotonically: `alpha.N` uses `0.0.0.N`, `beta.N` uses
`0.0.1.N`, and `rc.N` uses `0.0.2.N`. Replace it with `0.1.0` for the final
`v0.1.0` release.

Do not use `0.1.0.1` for an alpha of `0.1.0`: Windows Installer considers it
newer than the eventual `0.1.0` release.

## Repository Configuration

Install the SignPath GitHub App for `bianwoyali-design/RemCmd`, then create a
SignPath project and add the following repository configuration:

- Secret `SIGNPATH_API_TOKEN`: a SignPath token for a user permitted to submit
  signing requests for the release policy.
- Variable `SIGNPATH_ORGANIZATION_ID`: the SignPath organization ID.
- Variable `SIGNPATH_PROJECT_SLUG`: the SignPath project slug.
- Variable `SIGNPATH_SIGNING_POLICY_SLUG`: the policy used for release tags.
- Variable `SIGNPATH_ARTIFACT_CONFIGURATION_SLUG`: the artifact configuration
  used for the WiX MSI.

The workflow submits the MSI through a GitHub Actions artifact because SignPath
uses GitHub's origin metadata to verify the build. Configure the SignPath
artifact configuration as a ZIP-rooted artifact, then deep-sign both the MSI
and the embedded `remcmd.exe`. Upload a real unsigned MSI sample in SignPath to
generate the initial configuration, and review it before enabling the release
policy.

Do not use an artifact configuration that signs only the outer MSI. Windows
users also run the embedded executable after installation, so it must receive
an Authenticode signature as part of the same SignPath request.

## Release Behavior

- `workflow_dispatch`: unsigned MSI, DMG, DEB, and AppImage artifacts for test
  installation.
- `v*` tags: the Windows MSI is signed through SignPath and only the signed MSI
  is retained as `remcmd-windows`.
- macOS: the DMG remains unsigned until Developer ID signing and notarization
  are configured.
