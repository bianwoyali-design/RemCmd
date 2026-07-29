# Windows Code Signing

Release tags use WiX to create an MSI package. Pull requests and `main` builds
publish an unsigned MSI for installation testing. A `v*` tag must be signed by
SignPath; the workflow refuses to publish an unsigned Windows release.

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

- `pull_request`, `workflow_dispatch`, and pushes to `main`: unsigned MSI,
  DMG, DEB, and AppImage artifacts for test installation.
- `v*` tags: the Windows MSI is signed through SignPath and only the signed MSI
  is retained as `remcmd-windows`.
- macOS: the DMG remains unsigned until Developer ID signing and notarization
  are configured.
