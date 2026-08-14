# Diagnostics and Support Bundles

The **Settings → Diagnostics** view shows recent structured events and can
filter them by level, module, or text. It also opens the log directory, clears
logs after confirmation, temporarily enables detailed logging, and exports a
ZIP support bundle.

## Storage and retention

Diagnostics use JSON Lines files named by UTC date. The default level is INFO;
**Detailed logs for this run** enables DEBUG only until RemCmd exits. Daily
logs are retained for seven days. If the log directory cannot be initialized or
written, RemCmd continues running and keeps a bounded in-memory event buffer;
the Diagnostics view displays the failure.

RemCmd records application lifecycle, profile/import outcomes, SSH stages and
authentication methods, proxy/jump state, connection timing, SFTP operation
categories, and errors. It intentionally does not record terminal input or
output, Quick Commands, clipboard contents, remote file contents, or user file
data.

## Redaction

All events pass through one redactor before entering memory or disk. It removes
registered passwords, key passphrases, and proxy secrets, plus URI userinfo,
Authorization values, and common `password`, `passphrase`, `token`, and
`secret` patterns. Technical SSH and operating-system errors remain available
after redaction.

## Support bundle contents

The ZIP contains only redacted logs and a JSON manifest with the RemCmd
version, operating system, architecture, selected language, non-sensitive
settings, and anonymized connection/route shapes. Profile and endpoint IDs are
hashed. The bundle excludes raw ProxyCommand text, host credentials, detailed
private-key paths, terminal content, remote file content, and clipboard data.

Review the ZIP before sharing it. Redaction is deliberately layered and tested,
but no automatic process can infer every kind of sensitive text a remote system
might place inside an error message.
