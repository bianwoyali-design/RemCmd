# Windows Code Signing

The release workflow uses `cargo-packager` to create a WiX MSI. Until RemCmd
has a public release and is accepted by the SignPath Foundation, both manual
builds and `v*` tags publish unsigned Windows artifacts.

## MSI Versioning

WiX requires a numeric `major.minor.patch.build` version, whereas the
user-facing Cargo version may use SemVer prerelease labels. The WiX-only
`packaging/wix/windows.toml` therefore owns the MSI version. Its name avoids
`cargo-packager`'s automatic `Packager.toml` discovery, so the WiX-only version
cannot create an extra macOS or Linux package.

For `v0.1.0-alpha.1`, the Cargo version remains `0.1.0-alpha.1`, while the
MSI version is `0.0.0.1`. This keeps the alpha installer older than the future
`0.1.0` final installer. Before publishing another prerelease, update the
Windows version monotonically: `alpha.N` uses `0.0.0.N`, `beta.N` uses
`0.0.1.N`, and `rc.N` uses `0.0.2.N`. Replace it with `0.1.0` for the final
`v0.1.0` release.

The generated installer is renamed after packaging for release distribution,
so its filename remains user-facing SemVer, for example
`RemCmd-v0.1.0-alpha.1-windows-x86_64.msi`. Only the internal MSI
`ProductVersion` needs the numeric value.

Do not use `0.1.0.1` for an alpha of `0.1.0`: Windows Installer considers it
newer than the eventual `0.1.0` release.

## Future SignPath Configuration

After a public release makes the project eligible, install the SignPath GitHub
App for `bianwoyali-design/RemCmd`, create a SignPath project, and add the
following repository configuration:

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

## Current Release Behavior

- `workflow_dispatch`: unsigned Windows MSI, ad-hoc-signed macOS DMG, DEB,
  and AppImage artifacts for test installation.
- `workflow_dispatch` with `release_tag` set to an existing `v*` tag: rebuild
  the packages and create that tag's GitHub release. This is the recovery path
  for a failed release job without moving the tag.
- `v*` tags: create a GitHub prerelease when the tag contains a prerelease
  suffix and attach the unsigned Windows MSI, ad-hoc-signed macOS DMG, DEB,
  and AppImage.
- macOS: `cargo-packager` ad-hoc-signs the completed app bundle and the final
  DMG. This does not replace Developer ID signing or notarization.
