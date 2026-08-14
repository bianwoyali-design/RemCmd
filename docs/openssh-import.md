# OpenSSH Configuration Import

RemCmd can import connection profiles from the default `~/.ssh/config` or a
file selected in **Settings → Import OpenSSH Configuration**. Import is always
read-only: RemCmd never edits an OpenSSH configuration and does not synchronize
it automatically.

## Preview and re-import

Only positive, literal `Host` aliases become candidates. Wildcard blocks still
contribute defaults. The preview classifies every candidate as new, update,
unchanged, conflict, or invalid and shows source-file and line-number warnings.
You can select candidates, adjust authentication, and choose whether an
individual conflict keeps local values or uses the OpenSSH values.

`ProxyJump` dependencies are selected automatically. Re-import matches the
canonical root configuration path and alias, preserves the existing RemCmd
profile ID, and therefore keeps keychain references stable. A source-only
change can update directly. When both the imported source and the local profile
changed, RemCmd defaults to keeping the local profile until you explicitly
choose the OpenSSH values.

## Supported directives

RemCmd follows OpenSSH's first-obtained-value behavior for:

- `HostName`, `User`, `Port`, `IdentityFile`
- `ProxyJump` and `ProxyCommand`
- recursive `Include`, relative include paths, and glob expansion
- positive and negative `Host` patterns
- deterministic `Match host`, `originalhost`, `user`, `localuser`, and `all`
- documented home-directory, environment, and connection-token expansion

`Match exec` is never executed. Unsupported or unsafe conditions are ignored
with a warning. `ProxyUseFdpass` is not supported.

The alias is used as the profile name; `HostName` falls back to the alias;
port defaults to 22; and a missing `User` falls back to the local account. The
first `IdentityFile` selects private-key authentication and later identities
produce warnings. Without an identity file, macOS and Linux default to SSH
Agent while Windows defaults to password authentication.

## ProxyCommand security

Raw ProxyCommand text exists only in the in-memory preview and is written to
the operating system keychain when the import is applied. The profiles JSON
contains only a digest. If the keychain write fails, that candidate cannot be
applied; RemCmd does not fall back to JSON. Batch application atomically
replaces the profiles file and rolls back ProxyCommand keychain changes on a
normal failure path.
