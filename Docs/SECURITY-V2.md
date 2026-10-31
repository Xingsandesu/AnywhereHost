# Host security v2 — 2026-10-31

This is a breaking authentication upgrade for the pre-release product. Upgrade Host and iPhone together and scan a new QR. No v1 links, shared bearer credentials, TLS 1.2 transport, or authentication downgrade is accepted. Existing business endpoint paths remain `/v1/*`; the protocol/security version is 2.

## Trust and pairing

The QR contains `v=2`, `endpoint`, `ticket`, and `certificateSHA256` (plus optional `name`). A ticket is 256 random bits, valid for five minutes and usable by one device key. The Mac generates it using its private management authority; the backend persists only its SHA-256 digest. Generating another ticket replaces the previous one. While the pairing panel is open, it refreshes after four minutes or after enrollment is observed. The existing scan/save/connect interaction is unchanged.

An iPhone creates a P-256 signing key in Secure Enclave with `AfterFirstUnlockThisDeviceOnly` access; no biometric or extra confirmation is requested. Keychain stores the device-bound wrapped key representation. Physical iOS devices fail closed if Secure Enclave is unavailable; Simulator and macOS transport fixtures can use software keys. The Host stores public keys, never device private keys. Each device ID is SHA-256 of the 65-byte uncompressed SEC1 public key. Enrollment includes a signature proving possession of that key. A lost response may be retried with the same key only, and the client first checks whether enrollment already succeeded. Removing/replacing a saved Host cannot be undone by a late enrollment callback.

The management secret `admin-key-v2` is a separate random 256-bit secret, stored only on the Mac with mode 0600. It is never embedded in a QR, sent as a bearer, or available to the iPhone. Tunnel configuration, ticket generation, listing devices and revocation require management authority. Devices retain native Codex operations (including reconnect), APNs registration, rooted files and the locally enabled desktop proxy. All paired devices currently share the same workspace authority; there are no per-project roles.

## Transport and proofs

TLS is end-to-end through frpc's TCP forwarding, with TLS 1.3 required and 0-RTT disabled. Rustls AWS-LC offers hybrid X25519MLKEM768 preferentially where the client supports it; otherwise a modern TLS 1.3 group is negotiated. This does **not** establish that iOS negotiated a post-quantum group or that P-256 authentication is post-quantum.

Custom/self-signed certificates remain supported. Install the matching PEM certificate/chain and key as `host-cert.pem` / `host-key.pem` before starting Host. The exact leaf DER SHA-256 from the QR is the trust anchor, independently of DNS SAN or public CA status. There is no accept-any-certificate option. A certificate change requires a new scan. Redirects are refused. Users do not install a CA profile or manually trust certificates. The existing ATS exception permits these user-owned endpoints; URLSession still enforces TLS 1.3 and exact certificate pinning.

`GET /v1/security/challenge` is the sole unauthenticated resource. It exposes a random per-process challenge, server time and protocol version, no Host/runtime data. Clients cache it and use elapsed monotonic time to avoid phone clock skew. A restart creates a new challenge. Only explicit `challenge_expired` / `proof_expired` pre-dispatch rejection is retried; ambiguous writes are never automatically repeated.

Every other HTTP request and WSS upgrade signs these nine UTF-8 lines, without a trailing newline:

```text
Anywhere-v2
<lowercase leaf certificate SHA-256>
<exact Authorization value>
<HTTP method>
<percent-encoded path plus exact query>
<lowercase SHA-256 of exact body bytes, including empty body>
<decimal server-based Unix seconds>
<32 hexadecimal random request nonce>
<server boot challenge>
```

Headers are `Authorization`, `X-Anywhere-Content-SHA256`, `X-Anywhere-Time`, `X-Anywhere-Nonce`, `X-Anywhere-Challenge`, and `X-Anywhere-Signature`. Duplicate proof headers fail authentication. Authorization is `Anywhere-Device <deviceID>` or `Anywhere-Admin`. The signature is standard base64 of P-256/SHA-256 fixed-width 64-byte `r || s`, or HMAC-SHA256 with the ASCII management key respectively. Enrollment uses `Anywhere-Pair <ticket>` and `X-Anywhere-Public-Key` (standard base64 SEC1 public key), only on `POST /v1/security/pair` with an empty body. The signature envelope is an application-specific protocol, **not** RFC 9449 DPoP or RFC 9421 compliance.

Proofs are accepted within ±90 seconds of the monotonic server clock. Atomic replay tracking retains accepted identity/nonce pairs for 185 seconds; live entries are never evicted to admit new traffic. Capacity exhaustion fails closed. Proof authentication happens before body reading; the actual body digest must match before dispatch. The Host's pinned certificate, boot challenge, method, path, query and body are all bound to the signature.

## Revocation and resource limits

The Mac pairing panel lists abbreviated device IDs and can revoke a device. Revocation persists atomically, invalidates its consumed ticket, rejects future requests, and closes existing event/desktop streams on the next one-second check. Already dispatched Codex actions are not undone. Event replay is also cancellable during slow writes.

| Boundary | Limit |
| --- | --- |
| TCP connections, including TLS handshakes | 96 |
| TLS handshake / HTTP/1 header timeout | 5 s / 10 s |
| HTTP/1 headers / buffer | 48 / 32 KiB |
| HTTP/2 concurrent streams / header list | 32 / 16 KiB |
| In-flight application requests | 64 |
| Global request admission | 200/s, burst 400; no trust in forwarded IP headers |
| Signed body / body read deadline | 8 MiB / 15 s |
| Request target | 8 KiB |
| Paired devices / live replay entries | 32 / 32,768 |
| Event sockets / incoming event frame | 32 / 1 KiB |
| Event write deadline / receive inactivity | 10 s / 90 s |
| Desktop sessions / frame | 2 / 1 MiB |
| Desktop write deadline / receive inactivity | 15 s / 90 s |

Browser Origin (including `null`) is rejected on HTTP and WebSocket. Responses, including authentication failures, use `no-store`, `nosniff`, and `no-referrer`. Desktop credentials remain outside Host auth; Host keys never enter WebKit. RFB can reach only `127.0.0.1:5900`, and the local desktop switch must remain enabled. Keep macOS Screen Sharing itself restricted to trusted users/network exposure.

## Verification and boundaries

Run with a TLS 1.3-capable Python (`uv run --python 3.12 --no-project python …` on this machine; system LibreSSL Python cannot negotiate TLS 1.3):

- `cargo test`; `cargo clippy --all-targets -- -D warnings`; `swift build`; `swift test`.
- `Scripts/verify-host.py`: native protocol, stream replay, approval/permissions, rooted file/image protection and compression.
- `Scripts/verify-security.py`: self-signed TLS 1.3, TLS 1.2 refusal, enrollment, signature forgery, replay, route/query/method/body binding, privilege separation, revocation, persistence and restart rejection.
- `Scripts/verify-desktop.py`: relay frame/connection limits and desktop-switch revocation; requires unused loopback 5900 and refuses to replace an existing screen sharing service.
- iPhone repo: Swift transport smoke against two isolated Hosts, QR import, signature/Keychain tests and full native client suite.

### Recorded result on 2026-10-31

- Rust: 44 tests passed (40 Host + 4 permission probe); Clippy with warnings denied passed.
- Mac: build passed; 8 Swift tests passed; production `HostClient` management signatures and QR generation passed a pinned HTTPS smoke.
- Host HTTPS/WSS integration: 72 checks passed. Attack suite: 40 checks passed.
- Swift production transport: both isolated Hosts passed device enrollment, signed HTTPS/WSS, history/fork/turn/approval, multi-question answers, wrong-pin and unknown-device rejection.
- iOS 27.1 / iPhone Duo: 157 tests passed, 0 failed, 2 external-network tests skipped (163 executed parameterized cases). Result bundle: `/tmp/anywhere-security-final-result.xcresult`.
- Desktop: Rust's `signed_device_relay_preserves_bytes_and_revocation_closes_it` passed with a test-only ephemeral loopback peer; the real production target remains fixed at 5900. The standalone 5900 fixture was not run because the existing service owns the port. Native WKWebView RFB rendering/key tests passed. During verification a WebKit timer receiver error in the current desktop touch code was fixed by wrapping timer calls, preserving the ongoing desktop work.
- Desktop JavaScript: 13 tests passed; `npm audit --omit=dev` reported 0 known vulnerabilities. A RustSec dependency audit was not run (`cargo-audit` is not installed).
- No deployed Host was restarted, no TestFlight build was uploaded, and no existing Codex thread was mutated.

This protects the network trust boundary; it is not a claim of invulnerability, formal protocol verification, NIST AAL certification or completed penetration testing. A stolen unexpired QR can win first enrollment; the QR must stay on the trusted Mac screen. A compromised unlocked endpoint or another process running as the Mac user remains outside this boundary. Authentication does not limit Codex below its explicitly selected native permissions. Global limits bound resources but cannot guarantee availability against bandwidth exhaustion. Hardware Secure Enclave behavior and background APNs token rotation need physical-device validation; simulator success cannot prove them. TLS key rotation still requires re-pairing. No 2027 draft is treated as an established standard.

## Primary sources consulted

- [RFC 9852 (July 2026)](https://www.rfc-editor.org/info/rfc9852/): TLS 1.3 for new protocols; updates RFC 9325.
- [RFC 9325](https://www.rfc-editor.org/rfc/rfc9325.html): TLS configuration, forward secrecy and 0-RTT considerations.
- [NIST SP 800-63B-4](https://csrc.nist.gov/pubs/sp/800/63/b/4/final): authentication and replay-resistance principles; this implementation makes no AAL claim.
- [RFC 9449](https://www.rfc-editor.org/info/rfc9449/): sender-constrained credentials and nonce/replay design reference, not a conformance claim.
- [OWASP WebSocket Security](https://cheatsheetseries.owasp.org/cheatsheets/WebSocket_Security_Cheat_Sheet.html): Origin checks, revocation, message limits and resource boundaries.
- [Apple Secure Enclave](https://developer.apple.com/documentation/security/protecting-keys-with-the-secure-enclave) and [CryptoKit key access defaults](https://developer.apple.com/documentation/cryptokit/secureenclave/p256/signing/privatekey/init(compactrepresentable:accesscontrol:authenticationcontext:)): device-bound P-256 signing without added user prompts.
- [Rustls AWS-LC provider](https://docs.rs/rustls/latest/rustls/crypto/aws_lc_rs/index.html): supported TLS algorithms and key exchange groups.


## Local deployment — 2026-10-31 21:22 CST

Installed the Release bundle built from `2385ebd032bb0c4f427d6b3176f79d14958aae90`
at `/Applications/AnywhereHost.app`. All four installed executable hashes match
the verified packaged application. The prior app and complete Host state were
backed up under `~/Library/Developer/Xcode/Archives/2026-10-31/` as
`AnywhereHost-before-security-v2.app` and `AnywhereHost-before-security-v2-state`.
Existing configuration, certificate and APNs identity hashes remain unchanged.

The new service reports security protocol 2 and a connected `appRuntime` using
the existing Codex Home. Restarted the existing frpc tunnel with its saved settings;
a certificate-pinned TLS 1.3 signed status request through the public endpoint
succeeded. Legacy Bearer authentication returned HTTP 401. Remote desktop remains
enabled and loopback screen sharing is reachable. APNs sandbox/production remain
enabled, with no failed subscriptions reported. This does not establish physical
phone delivery or UI behavior.

Release checks: 44 Rust tests, strict Clippy, Swift build and 8 Swift tests passed;
72 integration checks passed against both the development and packaged backends;
40 security checks passed. The production Swift transport passed pairing and signed
HTTPS/WSS against two disposable packaged Hosts. Existing user conversations were
not mutated. Install receipt and logs are archived alongside the client 1.0 (24)
release artifacts. Clients must update and scan a new protocol-v2 QR code.


## Removal of obsolete v1 state — 2026-10-31

The installed Host now removes `access-token` during startup; a regression test
verifies that this does not rotate the v2 management key or TLS identity. Existing
v2 pairing, tunnel and APNs configuration remain intact. The former app/state
backups mentioned above have been intentionally deleted at the user's request.
The Host event journal and old Live Activity records were reset and vacuumed;
all 43 owned-thread IDs and the original Codex conversation store were preserved.
Public pinned TLS 1.3 signed status and desktop reachability passed after restart.

Old local archives, temporary fixtures and build caches were removed. Client
TestFlight builds 1–23 and the two available legacy Host TestFlight builds were
expired. Apple retains build records; expiration prevents further beta use and
is not deletion of Apple's records. Git history was explicitly retained.

The obsolete macOS sandbox container at
`~/Library/Containers/com.jintongshu.AnywhereHost/Data` could not be inspected or
removed: macOS returned `Operation not permitted`. SandboxProbe and
SandboxVerification data were removed, but OS-managed container metadata remains.
These limits prevent a claim that every historical byte has been erased.
