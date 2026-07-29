# macOS Installation

RemCmd DMGs use an ad-hoc signature. This keeps the bundle internally
consistent, but it does not provide Gatekeeper with a verified developer
identity and is not notarized.

Download builds only from the official RemCmd GitHub release. Control-click
the downloaded DMG, choose **Open**, then confirm **Open** in the dialog to
mount it. Move `RemCmd.app` to the Applications folder, then control-click the
app and choose **Open** again. If Gatekeeper blocks either the DMG or app, open
**System Settings > Privacy & Security** and choose **Open Anyway**.

This is expected for the current channel. RemCmd does not yet have a Developer
ID signature or Apple notarization.
