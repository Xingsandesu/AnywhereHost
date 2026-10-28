# Architecture and delivery plan

## Product architecture

AnywhereHost is the self-hosted backend for the iPhone Duo client. The Host protocol provides native Codex operations, structured workspace resources, and live execution state to the iPhone app.

The macOS deliverable is one application bundle containing a SwiftUI executable, a Rust service, and pinned upstream frpc. The user's installed Codex provides the external runtime.

## Components

```text
SwiftUI macOS console / iPhone Duo client
       │ HTTPS JSON + WSS events, Bearer + certificate pin
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

## Capability roadmap

| Area | 0.1 status | Remaining work for desktop parity |
| --- | --- | --- |
| Project identity and roots | Native list/read/create/update/move RPC; Mac list/create UI | Project rename/delete semantics and migrations |
| Threads and history | Native IDs, paginated history, start/resume/fork/archive/name RPC | Native iPhone sync/search implemented; pinning availability and conflict handling remain |
| Turns | Start/steer/interrupt RPC, events, Mac send/stop UI | iPhone queued/steering UX, background restoration |
| Context | Runtime settings, compaction, fork, goals/queues where supported | iPhone context/model/plan UI implemented; permissions profile selection, selected files and cross-device drafts remain |
| Approval and input | Bidirectional RPC, single-use replies, native command/file approval + questions | Rich MCP forms, complete permission schema forms, concurrent-device audit UX |
| Files and Git | Rooted list/text read, staged/unstaged diff API | iPhone file/Git reader implemented; editor, safe apply, conflict/merge UI remain |
| Attachments | Native attachment RPC exposed when supported | Phone-to-host upload, size/type policy, preview and cleanup |
| Desktop live process | Standard socket adapter implemented | Actual current desktop stdio session attachment; must use a supported shared endpoint |
| Worktrees | Not implemented | Safe creation, branch selection, artifacts, cleanup and ownership |
| Skills/plugins | Read/list RPC where available; inherited runtime config | Management UX, desktop-only injected plugin compatibility |
| Remote connectivity | Bundled frpc, pinned TLS, connection QR, local frps round trip tested | User frps deployment and real WAN/device validation, pairing codes, device revocation |
| macOS packaging | Single native-architecture .app, local signing | Developer ID, notarization, universal binary, updater, login lifecycle |
| iPhone Duo | Host v1 client, multi-computer/workspace SwiftUI, foreground Live Activity | APNs updates, uploads, worktrees, comprehensive parity and WAN validation |

A capability listed as an RPC is implemented as a gated native protocol operation; it is not a claim that every corresponding SwiftUI screen or every runtime version has been end-to-end validated.

## Sources

Protocol framing/lifecycle and transport guidance: [official Codex app-server documentation](https://learn.chatgpt.com/docs/app-server). Exact project, thread, approval, and queue fields were verified against schemas generated by the locally installed 0.162.0-alpha.2 binary. The installed schema takes precedence over older examples.

Tunnel configuration: [frp client reference](https://gofrp.org/en/docs/reference/client-configures/), [upstream frp v0.71.1 release](https://github.com/fatedier/frp/releases/tag/v0.71.1). TLS to frps and TLS to Host are separate connections; the forwarded Host port remains HTTPS end to end.
