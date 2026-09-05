# Windows Code Signing

The release workflow uses `cargo-packager` to create a WiX MSI. Until RemCmd
has a public release and is accepted by the SignPath Foundation, both manual
builds and `v*` tags publish unsigned Windows artifacts.

## MSI Versioning

Windows Installer compares only the first three numeric fields of `ProductVersion`;
a fourth field is ignored. The release validator enforces the following increasing
versions for the initial `0.1.0` release series:

| Cargo version | MSI ProductVersion |
|---|---|
| `0.1.0-alpha.N` | `0.0.N` |
| `0.1.0-beta.N` | `0.0.(10000 + N)` |
| `0.1.0-rc.N` | `0.0.(20000 + N)` |
| `0.1.0` | `0.1.0` |

`N` must be 1–9999. For example, beta.2 uses `0.0.10002` and rc.1 uses
`0.0.20001`. Published alpha.1 and beta.1 used legacy four-field versions;
new beta/RC packages compare newer than both. Do not rebuild or move their tags.
Define and test a new mapping before publishing another prerelease series.
Stable releases use their three-part Cargo version directly, within MSI limits.

The WiX-only `packaging/wix/windows.toml` owns the numeric version and avoids
`cargo-packager` auto-discovery. Filenames still carry the Cargo SemVer version.
Validate with `python3 scripts/release_metadata.py`; this also checks Cargo.lock.

See Microsoft's [ProductVersion rules](https://learn.microsoft.com/en-us/windows/win32/msi/productversion).

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
