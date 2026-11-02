# Verification

## Multi-platform GitHub CI (2026-11-02)

The `Build packages` workflow defines native macOS arm64 and Debian 13 amd64 / arm64 jobs for push, pull request and manual runs. `actionlint 1.7.12`, shell syntax and Python compilation checks passed locally. The first GitHub run passed Rust and Host protocol checks on all three targets, then exposed missing `vncpasswd` in source-only Linux tests and a Swift compiler crash in Xcode 26.6. Linux now runs the Python/GTK/desktop checks through the extracted complete package; Mac uses the project's Xcode 27 toolchain. Full package results are recorded by the corresponding Actions run.

On the local Debian 13 amd64 host, formatting, `cargo build --locked --bins`, 47 Rust tests, strict Clippy, 72 isolated Host protocol checks, 18 Python tests and six real frp fixture cases passed. Four new Python tests cover native release selection, missing signatures, foreign-architecture rejection at download/install and unsupported architectures. A fresh amd64 deb passed metadata and ELF architecture checks, extracted-package tests, GTK smoke and real TigerVNC framebuffer/input checks in isolated Xvfb. It was not installed over the running application.

The pinned Linux arm64 frpc archive was downloaded, its upstream SHA-256 matched, and its ELF header identified AArch64; that binary was not executed on this amd64 host. `swift build` could not run because Swift is not installed. CI's explicit unsigned Mac mode does not establish code-signing, notarization, Hardened Runtime, GUI launch or OTA acceptance. No live Codex inference or existing user threads were used.

## Debian 13 amd64 (2026-11-02)

- 47 Rust tests, strict Clippy and formatting checks passed; 72 isolated Host protocol checks passed.
- 10 Python management/update tests passed, including certificate pinning, signed pairing/revocation, external-process ownership, cancelled permission checks and update signature rejection.
- GTK smoke passed: eight management pages, setup gating/persistence, pairing/clipboard, permissions, background close/reopen, tray menu and explicit shutdown.
- Real TigerVNC in isolated Xvfb passed framebuffer, keyboard/pointer, independent authentication, private socket, signed WSS, two-session limit and disabling checks. Five real frpc/frps cases passed.
- The deb passed extracted-package checks and installation in a disposable Debian 13 container. The privileged updater rejected invalid signatures and installed a newer fixture-signed package in that container; no production signing key was used.
- An isolated headless GNOME Wayland session completed the real portal consent flow, independent VNC authentication and framebuffer transfer. Wayland keyboard/pointer acceptance remains for manual testing. An additional nested GNOME-on-Xvfb experiment crashed the nested compositor; it did not exercise the user's desktop.
- The live Codex smoke was rerun with shared-process thread/resume removed from the default read-only path. No inference was requested.

The Debian package is an unsigned development artifact. Production OTA needs a release signed with the existing Host identity. Physical-session portal persistence, real-device WAN use and full accessibility acceptance remain manual checks. Swift is unavailable on this Linux host, so the macOS build was not verified here.

Debian revision `0.1.4-2` fixes the shared frps proxy-name collision. The real frp fixture now runs two Hosts simultaneously against one frps and verifies both pinned HTTPS endpoints, distinct names, stable names after restart and migration from the old fixed name. All six frp cases, 47 Rust tests, strict Clippy and 72 Host checks passed again.

Debian revision `0.1.4-3` separates password saving from the sharing switch. Previously the only save path was the enable button, disabled when Host was offline, sharing unsupported or sharing already enabled. The dedicated save button works independently of those states and reports validation/save errors beside the password field. The original file writer succeeded in an isolated reproduction; the fix addresses the blocked UI save paths, not a reproduced file-generation failure.

For this fix, 14 Python tests, 47 Rust tests, strict Clippy and 72 isolated Host checks passed. GTK on isolated Xvfb verified saving with Host stopped, invalid-input feedback and changing the password while sharing is enabled. Real TigerVNC on isolated Xvfb accepted the new password and rejected the old password without restarting the helper, then passed framebuffer, keyboard/pointer, signed WSS, session-limit and disabling checks. The generated `0.1.4-3` deb passed extracted-package verification including those Python, Host, GTK and real desktop checks; it was not installed over the running application. No user desktop input or existing Codex threads were changed. Wayland consent was not rerun for this fix; `swift build` could not run because Swift is not installed.

## Source checks

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo build
python3 Scripts/verify-host.py
swift build
swift test
```

`verify-host.py` launches a disposable backend and deterministic Codex peer. It checks
HTTPS/WSS authentication, certificate pinning, capability gates, ownership, native
settings, history paging, approval correlation, events, compression, file boundaries
and process locking. It does not mutate existing user threads.

Swift tests cover setup persistence, directory-state mapping, required paths,
revocation, probe cancellation/timeouts, Host-only status requests and late-response handling.

## Packaged application

```sh
./Scripts/build-app.sh
python3 Scripts/verify-app.py dist/AnywhereHost.app --integration
python3 Scripts/verify-desktop.py --binary dist/AnywhereHost.app/Contents/MacOS/anywhere-host
```

The bundle checks cover all four executable signatures, Hardened Runtime, absence
of App Sandbox and the Apple Events entitlement/usage description. Desktop tests use
an isolated local byte service for authentication, forwarding, limits and revocation;
they do not enable system screen sharing or establish a real Mac login.

## Separate live checks

```sh
python3 Scripts/smoke-real-codex.py
python3 Scripts/verify-shared-codex.py
python3 Scripts/verify-frp.py
```

The default Codex smoke is read-only. Shared-socket checks use an isolated daemon;
frp checks use loopback servers. Neither proves attachment to the current desktop
process or connectivity from another network.

`smoke-real-codex.py --inference` explicitly opts into model use in an ephemeral
thread. `--first-message` creates a new persistent test thread and archives it afterward.
Neither belongs in the default read-only checks.

## Acceptance boundaries

Real-device WAN access, macOS consent dialogs/relaunch, full VoiceOver navigation,
APNs delivery, production screen sharing, cross-Mac installation and notarization
require separate validation. Screenshots and fixture checks do not establish these.

## 2026-10-31 interface refresh

Xcode 27.1 / macOS 27 SDK: `swift build`, 7 Swift tests, 35 backend tests plus
4 permission-probe tests, strict Clippy and 65 isolated HTTPS/WSS checks passed.
Offline native windows were rendered for visual inspection; no existing user thread
was changed. This is layout evidence, not live-service or full accessibility acceptance.

## 2026-10-31 工作区图片读取

新增经过同一 bearer 鉴权和 project/read 权威根目录校验的图片接口。`cargo test` 通过 36 项主程序测试和 4 项辅助测试；`cargo clippy --all-targets -- -D warnings`、`swift build` 通过。`Scripts/verify-host.py` 通过 72 项隔离 HTTPS/WSS 集成检查，新增图片字节一致性、无凭证拒绝、文本伪装图片拒绝、父目录与符号链接越界拒绝。未重启或替换用户运行中的 Host，使用该接口需要更新部署产物。

## 2026-10-31 Host-only Mac interface

The Mac conversation/project UI and its RPC, event, history, model and approval
state were removed. The dashboard reads only host-management endpoints; remote
Codex RPCs remain available to clients. Swift build and 8 Swift tests passed,
including management-only polling, offline-state clearing and stop/refresh races.
Rust: 36 backend + 4 permission-probe tests, strict Clippy and 72 isolated HTTPS/WSS
checks passed. Release packaging and all four executable signatures passed.
Light/dark offline dashboard windows were visually checked; no real user thread
was mutated by tests.

## Security v2 (2026-10-31)

See [security architecture and verification](SECURITY-V2.md): 44 Rust tests, 8 Mac tests, 72 HTTPS/WSS integration checks, 40 attack checks, dual-Host Swift proof interoperability, and independent signed desktop relay/revocation passed. Client: 157 tests passed, 2 external-network cases skipped. Custom self-signed TLS 1.3 remains supported; v1 bearer authentication is removed. Physical-device Secure Enclave/background delivery remains pending.

## 0.1.4 release verification (2026-10-31)

- Rust: 42 Host + 4 permission-probe tests; clippy all targets with warnings denied.
- Desktop regression includes idle pong, 4 MiB simultaneous send/receive, byte ordering and revocation; loopback random port, no real screen input.
- HTTPS/WSS fixture: 72 checks; security v2: 40 checks; frp fixture: forwarding, token reuse, conflict, auth failure and custom CA passed.
- Swift build and 9 tests passed. Packaged app signature, Hardened Runtime, common Sparkle Team ID, version and OTA metadata checked.
- Installed app UI displays `0.1.4 (7)` and Check for Updates. Actual native URLSession + local TLS pings passed for 25 seconds; public tunnel identity and status verified.
- Separate private client: busy-frame / silent-peer heartbeat regressions and actual WKWebView RFB handshake, pixels and input passed. Client changes are not part of this repository or Host release.
- Gitleaks scanned full Git history (21 pre-release commits) and the working directory with no findings.

Real phone drag/disconnect diagnosis is still pending user reproduction; the above is not a claim of end-to-end resolution.

Public release verification: `v0.1.4` is published in `Xingsandesu/AnywhereHost`.
Anonymous HTTPS downloads match SHA256SUMS and pass independent Ed25519 verification;
an altered ZIP is rejected. The installed app's Check for Updates reads the public
feed and reports 0.1.4 as current. The exact final release bundle is installed and
its saved tunnel restores to running after restart. Actual upgrade replacement
from a future lower Sparkle build is not yet exercised (0.1.3 had no updater).
