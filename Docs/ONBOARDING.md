# Host onboarding and permission checks

The first launch opens Welcome → Permission checks → Ready. On a fresh installation, the Host starts when the user begins setup. Existing configured installations keep serving paired devices while the new guide is shown. Completion requires successful read/write checks for required directories and a connected Codex runtime. Skipping does not persist completion, so the guide appears on the next launch. The overview, toolbar and menu bar expose permission checks; the overview and menu bar can reopen onboarding.

Directory checks use the packaged `anywhere-permission-probe --check-directory-access <path>` helper, before any server, state/configuration or Codex initialization. It enumerates at most one entry and creates, writes and removes a unique private temporary file. Existing file contents are not read or modified. Required paths are Host data, the actual runtime Codex Home, project roots and the user's chosen directory. Documents, Desktop and Downloads are optional unless also configured as project roots.

Each check runs in an owned subprocess with an eight-second timeout and cancellation, avoiding UI stalls on TCC prompts or disconnected volumes. The page distinguishes denied, read-only, missing, timed-out and unverifiable paths. Returning from System Settings refreshes system status and repeats previously requested file checks. After onboarding completion, startup rechecks required directories.

The page links to Files and Folders, Full Disk Access, Accessibility, Screen Recording, Automation and Local Network. Accessibility and screen recording have system preflight and explicit request buttons. Full Disk Access and Automation remain manual checks: a directory success is not presented as a complete system permission grant. Loopback connectivity does not establish local-network permission. Shared Codex and external tools may need separate consent and restart; Host results apply only to Host and its owned probe. Codex's sandbox and approval configuration are not rewritten.

The main app declares the Apple Events entitlement and usage description, including in CLI packaging. `Scripts/verify-app.py` checks these alongside Hardened Runtime and the existing no-App-Sandbox requirements. This enables normal user consent, not automatic authorization.

Validation commands:

- `swift build` and `swift test` (setup persistence, state mapping, required/optional paths, revocation, timeout and cancellation).
- `cargo test` and `cargo clippy --all-targets -- -D warnings` (including real directory reads/writes and permission-denied fixtures).
- `python3 Scripts/verify-host.py` against the fixture backend; no existing user threads are modified.
- Build the app and run `python3 Scripts/verify-app.py <app>` after signing.

Actual macOS consent dialogs, per-app authorization and revoke/relaunch behavior still require interactive verification with the intended signed app identity.

References: [Apple local network privacy](https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy), [Apple Events entitlement](https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.security.automation.apple-events).

The standalone permission probe allows a local UI update to retain a newer installed service binary without losing its capabilities. Local updates must compare capabilities and preserve pairing identity; record any retained binary in the installation report.
