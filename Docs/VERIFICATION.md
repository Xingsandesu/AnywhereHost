# Verification

## Direct distribution — 2026-10-29

- Removed App Sandbox and inherited-helper entitlements, folder bookmarks and
  authorization gates, runtime sandbox detection and `externalSandbox` overrides.
  Store upload instructions and the signed sandbox verification harness were removed.
- App and CLI use `~/Library/Application Support/AnywhereHost`. There is no
  legacy-container fallback, import or migration. The direct-distribution Host is
  now running from the standard state directory; paired-certificate verification
  confirmed the path. APNs credentials were explicitly configured there, and an
  isolated process successfully loaded both environments. The running process
  still needs a restart to load this configuration; no user thread data was copied.
- `cargo test --locked`: 23 tests passed. `cargo clippy --locked --all-targets --
  -D warnings`, `cargo build --locked` and `swift build` passed.
- `Scripts/verify-host.py`: 63 HTTPS/WSS checks passed. Tests observe the Codex
  peer's actual requests for default and explicit permissions through start, fork,
  resume, settings updates and turns; Host injects no extra policy calls.
- `Scripts/build-app.sh` produced a clean Release bundle. `Scripts/verify-app.py
  dist/AnywhereHost.app --integration` verified valid signatures and Hardened
  Runtime on all three executables, no App Sandbox/inheritance entitlements, no
  provisioning profile, and all 63 checks against the packaged Rust helper.
- Xcode Debug build with the project’s default manual ad-hoc signing also passed
  all three executable signature/entitlement checks. Explicit signing flags keep
  Hardened Runtime enabled for Xcode’s ad-hoc builds.
- Real installed Codex read-only smoke, isolated shared-socket attachment and
  loopback frpc/frps checks passed, including saved settings, token reuse, failed
  authentication, port conflicts and custom CA verification.
- This verifies build/signing/protocol behavior. Native UI acceptance, real-device
  APNs delivery, Developer ID notarization and cross-Mac installation were not
  performed in this change.

## Earlier verification — 2026-10-28

## Empty thread lifecycle regression

- Reproduced the reported thread using read-only live calls: metadata exists, Host owns it and `canMutate:true`, but `thread/turns/list` reports the precise pre-first-message error. Existing user threads were not mutated.
- The compatibility adapter converts only that error on a first page into empty history. Rust unit tests cover incorrect IDs, cursor reads, different methods, unrelated failures and successful-page pass-through.
- `cargo test --locked`: 8 tests passed; clippy with `-D warnings` passed; `Scripts/verify-host.py`: 34 HTTPS/WSS integration checks passed; `swift build` passed. The standalone `.app` was rebuilt and its signature verified. The already-running production Host has not been restarted; client compatibility with its original error response is retained.
- `Scripts/smoke-real-codex.py --first-message` explicitly creates a new persistent test thread in a temporary workspace using the installed App runtime. Metadata/access/empty-history read, direct first-message submission, real streamed reply and paginated persisted history passed. Only the newly created test thread was archived afterward. This is distinct from the default read-only smoke and the existing ephemeral `--inference` option.
- Real protocol checks exposed an additional client bug: `thread/resume` before the first message fails because no rollout exists yet. The native iOS client now starts that first turn without resume; changing model settings also avoids premature resume.

Environment: this Apple Silicon Mac, Swift 6 / installed Xcode SDK, Rust 1.99.1, Codex App runtime `0.162.0-alpha.2`, frpc/frps `0.71.1`.

| Check | Result |
| --- | --- |
| Rust unit tests | 6 passed: journal persistence/epoch, retention resync, structured approval validation, frpc serialization, diagnostic redaction, root traversal/symlink escape |
| `cargo clippy --all-targets -- -D warnings` | Passed |
| Real TLS HTTP/WSS fixture | 30 checks passed; launches the actual Rust service and a deterministic isolated Codex protocol peer |
| Real installed App runtime | Initialize, 3 actual projects, thread list, paginated history, 8 model entries and project file listing passed |
| Real inference | Ephemeral thread in a temporary directory streamed the requested marker and emitted matching completed turn; no tools requested, no existing user threads changed |
| Real shared Unix socket | Separate isolated Codex daemon attached through native WebSocket; Host shutdown preserved the external daemon |
| Real frpc/frps | Default TLS and explicit CA both forwarded pinned HTTPS through local frps; restart/config recovery, credential reuse, wrong-token failure, port conflict and start/status/stop passed |
| Swift debug and release builds | Passed |
| Single application packaging | `.app` includes SwiftUI executable, Rust executable, frpc, Info.plist and upstream license; `codesign --verify --deep --strict` passed with local ad-hoc signing |
| Application launch | Packaged SwiftUI app started bundled backend; authenticated local status reported connected to App runtime |

`Scripts/verify-host.py` checks missing/incorrect bearer, browser-origin rejection, WebSocket authentication before upgrade, private token generation, exact TLS fingerprint, rejection without trust/pinning, real process request framing, desktop source filtering, foreign-thread protection, unsupported arbitrary command methods, cwd validation, ownership retention, rollout-path replacement rejection, turn start, pending approvals, malformed/duplicate approval handling, correlated completion, WSS replay, invalid epochs, rooted file reads, path/symlink escapes and single-instance locking.

`Scripts/smoke-real-codex.py` defaults to reads against the real installed environment. `--inference` explicitly opts into a small model call. `Scripts/verify-shared-codex.py` uses a disposable Codex Home and tests a shared daemon; it does not establish that the current desktop App exposes its private stdio transport. `Scripts/verify-frp.py` only listens on loopback and does not connect to a user or public frps server.

Not validated: a real iPhone Host v1 connection, WAN reachability from the phone, APNs/background behavior, current desktop App private-process attachment, every exposed advanced RPC, simultaneous independent desktop/Host writes, notarized distribution, Intel builds, multi-device races beyond single-use approval handling, full macOS accessibility/UI acceptance. A development view snapshot was inspected, but native macOS compositing was incomplete; it is not treated as UI acceptance evidence.

## Native iPhone client integration (2026-10-28)

The adjacent AnyWhereCoding client now connects with pinned Swift URLSession HTTPS/WSS and structured actions. A new additive `threadAccess` capability and `GET /v1/threads/{id}/access` endpoint expose the existing ownership/shared-process gate for its read-only/fork UI. Host integration coverage increased from 27 to 30 checks, including the advertised capability and foreign/owned thread state; Rust tests and clippy pass.

Two disposable Rust Hosts with isolated fake Codex peers were exercised from the client transport (history, fork, turn, approval, completion; wrong pin/token rejected). The same Swift transport read the actual installed Codex runtime's 3 projects and 8 models without mutating existing threads. The macOS bundle was rebuilt and restarted while it had zero loaded threads and zero pending requests. iPhone Duo UI verification and detailed client limitations are tracked in the client repository's `Docs/HostClient.md`.

The iOS HostSession integration test also passed against both disposable services, requiring a WSS-only token-usage event. Native Duo simulator interaction verified multiple computers/workspaces, read-only history and explicit fork, approvals, completion, status and file browsing. This is simulator evidence, not physical iPhone or WAN validation.

## frps connection fix (2026-10-28)

The previous no-CA configuration emitted `transport.tls.useSystemCa`, which the bundled frpc 0.71.1 rejects before connecting. The existing local configuration also had the frps control port and desired proxy port confused. The renderer now uses upstream's standard TLS mode when no CA is supplied; an explicit CA continues to verify frps identity. End-to-end Host HTTPS pinning is unchanged. The UI separates control and proxy ports, restores saved settings without retrieving the token, and displays bounded diagnostics. Verification failures retain their actual error instead of a generic message.

`Scripts/verify-frp.py` now covers the previously missing default-certificate server and no-CA client path, as well as explicit CA validation, persisted settings across a Host restart, scoped credential reuse, failed authentication, occupied public ports, private config/log permissions and redacted status. Server readiness is checked by its loopback listener rather than a fixed startup delay.

After rebuilding and restarting the packaged app, a real connection to the user's supplied frps authenticated and registered the Host TCP proxy. This Mac reached the public HTTPS endpoint through frps: its leaf certificate exactly matched the local Host certificate; missing credentials returned HTTP 401; authenticated status returned HTTP 200 and a connected Codex App runtime. This is a real public-route check from the Mac, not a physical iPhone network test. No production thread was mutated.
