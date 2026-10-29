# macOS packaging

- Bundle ID: `com.jintongshu.AnywhereHost`
- Developer team: `775Y7KV7KU`
- App Store Connect app: `6827936120`
- Version: `0.1.0 (3)`
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
accept and process the upload. The Host uses App Sandbox with persistent folder grants and the installed
Codex runtime. See [sandbox setup and execution verification](SANDBOX.md).

## Validation

The Release archive and App Store Connect package export succeed. The application
and both embedded executables pass signature verification. On 2026-10-29, the
14 Rust tests, 44 HTTPS/WSS integration checks, Clippy and Swift package build
passed. The real loopback frpc/frps regression also passed for default TLS,
explicit CA verification, saved settings, credential reuse, wrong tokens and
occupied ports. The ICNS decodes to all ten required pixel representations,
including the 1024-pixel image. Individual archive upload results are recorded below.

## TestFlight validation

On 2026-10-28, Apple created app record `6827936120` for
`com.jintongshu.AnywhereHost`. Uploading `0.1.0 (1)` with the existing Xcode
account reached server validation and failed with **App sandbox not enabled**.
The rejection names all three executables: `AnywhereHost`, `anywhere-host`, and
`frpc`. Signing and account authentication are working; this build is not
available in TestFlight.

Build `0.1.0 (2)` adds sandbox signatures for all three executables, persistent
folder authorization, Full Disk Access guidance and native Codex external-sandbox
execution. Its real command/file and frpc checks are recorded in [SANDBOX.md](SANDBOX.md).
Upload acceptance and processing are recorded separately below. Apple also reported
missing dSYMs for the Rust and frpc executables as symbol-upload warnings in build 1.


## Build 2 upload result

On 2026-10-28 at 21:28 CST, Xcode reported **Upload succeeded** and
**EXPORT SUCCEEDED** for `com.jintongshu.AnywhereHost`, `0.1.0 (2)`.
Apple accepted all three sandboxed executables and reported that the uploaded
package had begun processing. This is upload acceptance, not confirmation that
processing or tester availability has completed. The non-blocking Rust/frpc dSYM
warnings remain.

The archive is `AnywhereHost-TF-0.1.0-2.xcarchive` in the dated Xcode Archives
directory; an adjacent `.upload.json` records the result and log location. It was
built from the working tree, including the pre-existing tunnel changes. The
sandbox commit is scoped to sandbox adaptation, signing, verification and docs.

## Build 3 upload result

On 2026-10-29 at 10:37 CST, Xcode reported **Upload succeeded** and
**EXPORT SUCCEEDED** for `0.1.0 (3)`, built from commit `5b10eb2`.
This release includes gzip responses, replay-safe text offsets, restored frp
settings and connection diagnostics. The previously uncommitted tunnel work is
now included in commit `edd2b3f`; the release working tree was clean.

The optimized Rust build passed all 44 HTTPS/WSS checks and has the same Mach-O
UUID as the archived helper. The inherited-sandbox helper cannot be launched
directly from an unsandboxed shell; that attempted invocation did not run the
fixture. Sandbox behavior remains covered by the signed-app verification
recorded in [SANDBOX.md](SANDBOX.md), and all three archive signatures and
entitlements were verified. No production Host process was replaced.

The archive and adjacent upload receipt are under
`~/Library/Developer/Xcode/Archives/2026-10-29/AnywhereHost-TF-0.1.0-3.*`.
Rust/frpc dSYM warnings remain non-blocking. The authenticated TestFlight page
subsequently showed build 3 under version 0.1.0, associated with the internal
group and two tester invitations. Processing is complete; the displayed beta
submission state is “准备提交”, matching the existing internally distributed build 2.
