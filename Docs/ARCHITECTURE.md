# Architecture

## Product architecture

AnywhereHost is a self-hosted backend for Host-compatible clients across devices. Its macOS and Linux applications manage the host service; Codex interaction belongs to clients. The protocol exposes Codex operations, structured workspace resources, and live execution state.

The macOS deliverable is one application bundle containing a SwiftUI executable, a Rust service, and pinned upstream frpc. The Linux deliverable is a Debian 13 amd64 package containing a GTK 4/PyGObject console, the same Rust service and pinned Linux frpc. The user's installed Codex provides the external runtime.

## Components

```text
SwiftUI / GTK host management / Host security v2 clients
       │ HTTPS JSON + WSS events, device signatures + certificate pin
       ▼
Rust Host: capability gate / request correlation / event journal
       ├── explicit project file and Git readers
       ├── pending approval and user-input registry
       ├── frpc supervisor → user's frps → encrypted Host traffic
       └── Codex protocol transport
            ├── existing control socket via native WebSocket over Unix
            ├── installed App runtime, same CODEX_HOME
            └── installed CLI fallback, same CODEX_HOME
```

Only process startup and the fixed login environment probe use process arguments. User actions use structured JSON protocol methods. Text input is a message, never interpreted by Host as a slash command. Git operations have fixed arguments, project roots from Codex, no shell expansion, no external diff/textconv programs, bounded output, and timeouts.

## Identity and context

The authoritative objects are Codex project IDs, thread IDs, turn IDs and item IDs. Project roots and thread cwd remain absolute paths on the host. Never rebuild a conversation by sending its transcript as a new prompt. `thread/resume` must use the original ID; path/history replacement is rejected. Context compression, fork, model/settings updates and queues use native protocol methods. Original sandbox/approval policy is not automatically weakened.

Existing standard control sockets can share loaded state and the owning process's environment. An app-bundled binary started independently shares disk state and login/configuration but cannot automatically share the Electron frontend, active turns, desktop-injected tools, or ephemeral environment changes made after that desktop process started. Status distinguishes these modes, and the private-process fallback refuses mutations to threads it did not create. Cross-process ownership for Host-created threads opened in other clients remains a limitation.

Installed protocol schemas are generated into temporary files at startup. Host does not hardcode a model catalog and does not promise methods absent from the runtime. RPC errors propagate with code/message/data; no mutation is automatically retried after timeout, because its outcome may be unknown.

## Events and lifecycle

One Host-owned connection multiplexes native RPCs using unique request IDs. Server requests use a separate opaque Host request ID and are answered at most once. Unsupported client-side tool/auth capabilities receive an explicit method-not-supported error. A disconnected runtime fails in-flight requests and clears live approval state. Reconnection is explicit and does not silently restart or stop an external daemon.

Events are journaled in SQLite with a persistent epoch and monotonic sequence. Subscribe before replay, deduplicate by sequence, and surface expired cursors with a resync requirement. Retention is the most recent 5,000 messages, not unlimited full-text history. Full thread history is always fetched from Codex. The stream is shared by devices using the Host credential; no per-device isolation is claimed.

The application stops only its own backend, frpc and app-server children. An OS file lock prevents two backend processes from owning the same Host state directory. The Host service must remain running; this release has no launch-at-login service or crash supervisor.

## macOS interface

Design references: [Apple HIG materials](https://developer.apple.com/design/human-interface-guidelines/materials),
[sidebars](https://developer.apple.com/design/human-interface-guidelines/sidebars), and
[SwiftUI Liquid Glass](https://developer.apple.com/documentation/swiftui/applying-liquid-glass-to-custom-views).

The macOS app is a single host dashboard with service status, device pairing,
tunnel settings, remote desktop, permissions, logs and start/stop controls. Native
toolbars and grouped settings adopt the current system appearance; primary actions
use Liquid Glass on macOS 26+. The app polls only Host status, capabilities, tunnel
and desktop endpoints. It has no project/thread browser, message composer, model
picker, approval form, native RPC client or conversation event subscription.

First launch offers Welcome → Permissions → Ready. Completion requires successful
read/write checks of required directories and a connected Codex runtime. Skipping
does not persist completion. Configured installations keep serving while setup is
shown. Checks use an owned permission-probe subprocess with an eight-second timeout
and cancellation. It enumerates at most one entry and creates, writes and removes
one private temporary file; existing file contents are not modified.

The permissions page exposes macOS settings and actual Host accessibility/screen
recording preflight results. A successful directory check is not a Full Disk Access
grant. Host checks do not establish permissions for a shared Codex process or external
tool. Required directories are rechecked at startup after setup completion. Checks cover
Host data, the actual Codex Home and an explicitly selected directory; the Mac app
does not enumerate Codex projects.

## Linux interface

See [Debian 13 / GTK](LINUX.md). The console manages service lifecycle, signed/pinned management requests, pairing and device revocation, frp, runtime paths, directory access checks and logs. Network and process waits run outside the GTK thread. It uses XDG state paths and preserves the installed Codex Home. Closing the GTK window keeps the service running; the application menu and StatusNotifierItem provide explicit quit. The three-step setup and permission checks gate completion on actual directory access and Codex connectivity. Signed deb updates use the existing Ed25519 release identity and a Polkit installer that re-verifies a root-owned copy. Wayland/X11 sharing uses pinned TigerVNC helpers supervised by Rust over a private Unix socket, with portal consent and independent VNC authentication. Capabilities reflect the installed helper and active desktop session. No systemd or launch-at-login service is installed.

## Implemented surface and limits

- Native RPC: projects, threads, history, turns, settings, approvals and catalogs,
  gated by installed runtime support. See the allowlist in `Docs/API.md`.
- Mac UI: host service status and lifecycle, pairing, tunnel settings, remote desktop,
  permission checks and logs. Codex conversation and approval UI belong to clients.
- Resource APIs: rooted file listing/text, Git diff, events, tunnels, APNs registration
  and desktop proxy. An API does not imply an equivalent Mac editor or management UI.
- Pending: worktree management, file editing, rich MCP forms,
  desktop private-process attachment and Developer ID notarization. Sparkle signed OTA is implemented; see DISTRIBUTION.md.

Client interfaces and platform-specific features are documented in their own
repositories; Host support does not establish complete desktop parity.

## Security

[Security v2](SECURITY-V2.md) implements single-use QR enrollment, Secure Enclave device proofs, separate management authority, TLS 1.3, revocation and bounded ingress. Custom/self-signed certificates remain supported by exact leaf pinning.

## Sources

Protocol framing/lifecycle and transport guidance: [official Codex app-server documentation](https://learn.chatgpt.com/docs/app-server). Exact project, thread, approval, and queue fields were verified against schemas generated by the locally installed 0.162.0-alpha.2 binary. The installed schema takes precedence over older examples.

Tunnel configuration: [frp client reference](https://gofrp.org/en/docs/reference/client-configures/), [upstream frp v0.71.1 release](https://github.com/fatedier/frp/releases/tag/v0.71.1). TLS to frps and TLS to Host are separate connections; the forwarded Host port remains HTTPS end to end.
