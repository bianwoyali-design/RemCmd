# Proxies and Jump Hosts

Each connection can use one upstream proxy and an ordered list of existing
RemCmd profiles as jump hosts. Existing profiles without routing metadata stay
direct, so older `profiles.json` files remain compatible.

## Proxy modes

- **HTTP CONNECT** supports no authentication or Basic username/password.
- **SOCKS5** supports no authentication or username/password and asks the proxy
  to resolve the destination hostname.
- **ProxyCommand** runs through the platform shell and uses its standard input
  and output as the SSH byte stream. It expands `%%`, `%h`, `%n`, `%p`, and
  `%r`.

HTTP CONNECT and SOCKS5 can reach either the target or the first jump host.
ProxyCommand and jump-host lists are mutually exclusive. HTTPS proxies,
SOCKS4, NTLM/Kerberos proxy authentication, and `ProxyUseFdpass` are not
supported.

Proxy passwords and raw ProxyCommand text are stored only in the operating
system keychain. They are never serialized in profiles. RemCmd limits captured
ProxyCommand standard error and terminates its child process when connection
setup is cancelled, fails, or disconnects.

## ProxyCommand approval

Before first execution, RemCmd displays the fully expanded command and a risk
warning. Approval is a SHA-256 digest of the command plus the current endpoint
name, host, port, and username. Editing any of those values invalidates approval
and requires confirmation again. Neither the raw command nor its expanded form
is written to diagnostic logs.

## Jump chains

Jump hosts form a complete ordered chain. A referenced profile contributes its
host, port, username, and authentication configuration; its own route is not
expanded recursively. A profile cannot reference itself or repeat a jump host,
and a referenced profile cannot be deleted until its references are removed.

Every hop performs its own host-key verification and authentication. After a
hop connects, RemCmd opens an SSH `direct-tcpip` stream to the next endpoint.
The final transport is shared by the interactive terminal, SFTP, and
performance monitor. On failure, errors identify **Proxy**, **Jump N/total**,
or **Target**, and established transports and proxy processes close in reverse
order.

Passwords and passphrases are requested per step. A remembered value is saved
only after that step authenticates successfully; a rejected saved value removes
only the matching step's keychain item before retry.
