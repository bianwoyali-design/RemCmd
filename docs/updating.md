# Updating RemCmd

Open **Settings → Software Update**, choose **Check for Updates…** in the
application menu (Help on Windows), or use the button in the About window.

RemCmd checks the official GitHub repository for a newer stable release. It
compares semantic versions, never offers an older version or a prerelease, and
selects the package for the current operating system and architecture.

## Download and install

1. Choose **Download Update**. Progress is shown in the update panel; cancel or
   retry if needed.
2. RemCmd checks the downloaded size and SHA-256 digest supplied by GitHub. A
   partial or invalid download never becomes an installable file.
3. Choose **Open Download**. RemCmd verifies the file again before opening it.
4. On macOS, use the DMG to replace the application. On Windows, follow the MSI
   installer. On Debian/Ubuntu, open the DEB with the package installer.
5. Restart RemCmd after installation. Saved profiles, settings and keychain
   credentials remain in their existing locations.

For AppImage installations, RemCmd reveals the verified download. Replace the
old AppImage, add executable permission to the new file, and launch it. A running
AppImage keeps using the AppImage package even on Debian-based systems.

These are installer-assisted updates, not a silent replacement of the running
application. Follow the normal save/close prompts before restarting. Download
verification checks integrity; it does not substitute for platform code signing
or notarization. The documented unsigned/ad-hoc installation limitations still
apply until signing is enabled.

## Automatic checking and privacy

Automatic checking is enabled by default and can be disabled in Software Update.
The app checks on startup when due and checks again while it stays open, at most
once per day. Manual checks are always available. Attempts are recorded so an
unavailable network does not cause a request loop.

Requests go to GitHub's public release API and include the RemCmd version in the
User-Agent. No connection profiles, host addresses or credentials are sent.
Download redirects are restricted to GitHub's HTTPS release hosts. Verification
failures leave the current installation unchanged.

If GitHub is unavailable, no stable release exists yet, the current platform has
no package, or verification information is missing, the panel explains the state
and offers the official release page. Downloads use the `updates` subdirectory
of the normal RemCmd application data directory.
