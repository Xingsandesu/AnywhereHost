# macOS packaging

- Bundle ID: `com.jintongshu.AnywhereHost`
- Developer team: `775Y7KV7KU`
- App Store Connect app: `6827936120`
- Version: `0.1.0 (1)`
- Target: Apple Silicon, macOS 14 or later.

`AnywhereHost.xcodeproj` builds the SwiftUI application and embeds the Rust Host,
checksum-verified frpc, its license, and the macOS application icon. Version and
bundle metadata come from the Xcode target settings. The Swift package remains
available for source compilation.

## Icon

The monochrome display and code-bracket mark is drawn by
`swift Scripts/render-icon.swift`. It produces `Resources/AppIcon.png`, the ten
standard macOS icon representations, and `Resources/AppIcon.icns`. Xcode embeds
the ICNS in the application bundle and declares it in Info.plist.

## Local application

```sh
./Scripts/build-app.sh
open dist/AnywhereHost.app
```

The script builds through Xcode, assembles the app in `dist`, signs the three
executables and bundle, and verifies their signatures. Local signing defaults to
ad hoc; `ANYWHERE_SIGNING_IDENTITY` selects a locally available identity.

## Archive and upload

```sh
asc xcode archive --project AnywhereHost.xcodeproj --scheme AnywhereHost \
  --configuration Release --archive-path /tmp/AnywhereHost.xcarchive \
  --xcodebuild-flag=-destination --xcodebuild-flag=generic/platform=macOS \
  --xcodebuild-flag=-allowProvisioningUpdates

asc xcode export-options generate --archive-path /tmp/AnywhereHost.xcarchive \
  --destination upload --output-path /tmp/AnywhereHost-Upload.plist

asc xcode export --archive-path /tmp/AnywhereHost.xcarchive \
  --export-options /tmp/AnywhereHost-Upload.plist \
  --xcodebuild-flag=-allowProvisioningUpdates
```

The Apple account must have access to the app record and signing team. A local
archive or signed package does not establish TestFlight availability; Apple must
accept and process the upload. The Host currently runs outside App Sandbox to use
the installed Codex runtime and workspace tools. Sandbox compatibility is a
separate requirement for the App Store distribution path.

## Validation

The Release archive and App Store Connect package export succeed. The application
and both embedded executables pass signature verification. The archived Rust
executable passes the 34 HTTPS/WSS integration checks with isolated test data;
the Rust suite, Clippy and Swift package build also pass. The ICNS decodes to all
ten required pixel representations, including the 1024-pixel image.

## TestFlight validation

On 2026-10-28, Apple created app record `6827936120` for
`com.jintongshu.AnywhereHost`. Uploading `0.1.0 (1)` with the existing Xcode
account reached server validation and failed with **App sandbox not enabled**.
The rejection names all three executables: `AnywhereHost`, `anywhere-host`, and
`frpc`. Signing and account authentication are working; this build is not
available in TestFlight.

The app's use of the installed Codex runtime, Codex Home, project files, and
spawned tools needs a tested App Sandbox design before another TestFlight
attempt. Adding the entitlement alone does not establish that those workflows
continue to work. Apple also reported missing dSYMs for the Rust and frpc
executables as symbol-upload warnings.
