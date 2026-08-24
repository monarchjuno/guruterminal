# Install Guru Terminal

Guru Terminal is installed as a desktop application, not as a Codex or Claude plugin.

## macOS 13 or newer on Apple Silicon

1. Download the signed `.dmg` for `aarch64-apple-darwin` from the latest `monarchjuno/guruterminal` GitHub Release.
2. Verify the published SHA-256 checksum.
3. Drag Guru Terminal to Applications and launch it normally. The release must pass Apple notarization and stapling checks.

## Windows x64

1. Download the signed NSIS `.exe` for `x86_64-pc-windows-msvc` from the latest GitHub Release.
2. Verify the published SHA-256 checksum and Authenticode signature.
3. Run the installer and launch Guru Terminal from the Start menu.

## Updates

Guru Terminal checks the stable signed update feed at most once per day and also provides **Check for updates** in Settings. Download, installation, and restart always require user confirmation. Prereleases are never offered on the stable channel.

Guru Terminal V1 does not import pre-1.0 state. Existing files are left untouched.
