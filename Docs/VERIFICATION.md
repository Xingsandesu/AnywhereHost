# Verification — 2026-10-28

Environment: this Apple Silicon Mac, Swift 6 / installed Xcode SDK, Rust 1.99.1, Codex App runtime `0.162.0-alpha.2`, frpc/frps `0.71.1`.

| Check | Result |
| --- | --- |
| Rust unit tests | 5 passed: journal persistence/epoch, retention resync, structured approval validation, frpc serialization, root traversal/symlink escape |
| `cargo clippy --all-targets -- -D warnings` | Passed |
| Real TLS HTTP/WSS fixture | 30 checks passed; launches the actual Rust service and a deterministic isolated Codex protocol peer |
| Real installed App runtime | Initialize, 3 actual projects, thread list, paginated history, 8 model entries and project file listing passed |
| Real inference | Ephemeral thread in a temporary directory streamed the requested marker and emitted matching completed turn; no tools requested, no existing user threads changed |
| Real shared Unix socket | Separate isolated Codex daemon attached through native WebSocket; Host shutdown preserved the external daemon |
| Real frpc/frps | Bundled frpc forwarded authenticated, pinned HTTPS through a local-only frps; start/status/stop passed |
| Swift debug and release builds | Passed |
| Single application packaging | `.app` includes SwiftUI executable, Rust executable, frpc, Info.plist and upstream license; `codesign --verify --deep --strict` passed with local ad-hoc signing |
| Application launch | Packaged SwiftUI app started bundled backend; authenticated local status reported connected to App runtime |

`Scripts/verify-host.py` checks missing/incorrect bearer, browser-origin rejection, WebSocket authentication before upgrade, private token generation, exact TLS fingerprint, rejection without trust/pinning, real process request framing, desktop source filtering, foreign-thread protection, unsupported arbitrary command methods, cwd validation, ownership retention, rollout-path replacement rejection, turn start, pending approvals, malformed/duplicate approval handling, correlated completion, WSS replay, invalid epochs, rooted file reads, path/symlink escapes and single-instance locking.

`Scripts/smoke-real-codex.py` defaults to reads against the real installed environment. `--inference` explicitly opts into a small model call. `Scripts/verify-shared-codex.py` uses a disposable Codex Home and tests a shared daemon; it does not establish that the current desktop App exposes its private stdio transport. `Scripts/verify-frp.py` only listens on loopback and does not connect to a user or public frps server.

Not validated: a real iPhone Host v1 connection, WAN reachability, APNs/background behavior, current desktop App private-process attachment, every exposed advanced RPC, simultaneous independent desktop/Host writes, notarized distribution, Intel builds, multi-device races beyond single-use approval handling, full macOS accessibility/UI acceptance. A development view snapshot was inspected, but native macOS compositing was incomplete; it is not treated as UI acceptance evidence.

## Native iPhone client integration (2026-10-28)

The adjacent AnyWhereCoding client now connects with pinned Swift URLSession HTTPS/WSS and structured actions. A new additive `threadAccess` capability and `GET /v1/threads/{id}/access` endpoint expose the existing ownership/shared-process gate for its read-only/fork UI. Host integration coverage increased from 27 to 30 checks, including the advertised capability and foreign/owned thread state; Rust tests and clippy pass.

Two disposable Rust Hosts with isolated fake Codex peers were exercised from the client transport (history, fork, turn, approval, completion; wrong pin/token rejected). The same Swift transport read the actual installed Codex runtime's 3 projects and 8 models without mutating existing threads. The macOS bundle was rebuilt and restarted while it had zero loaded threads and zero pending requests. iPhone Duo UI verification and detailed client limitations are tracked in the client repository's `Docs/HostClient.md`.

The iOS HostSession integration test also passed against both disposable services, requiring a WSS-only token-usage event. Native Duo simulator interaction verified multiple computers/workspaces, read-only history and explicit fork, approvals, completion, status and file browsing. This is simulator evidence, not physical iPhone or WAN validation.
