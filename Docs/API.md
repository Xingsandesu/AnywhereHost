# AnywhereHost API v1

Base URL: `https://host:port`. Native clients authenticate **every** HTTP request and WebSocket upgrade with `Authorization: Bearer <host-token>`. Credentials in query strings are unsupported. Browser `Origin` requests and redirects are rejected. Responses are not cacheable.

The connection QR encodes:

```text
anywhere-host://connect?v=1&endpoint=<percent-encoded-https-url>&token=<64-hex-token>&certificateSHA256=<64-hex-fingerprint>
```

Verify the actual leaf certificate DER SHA-256 against this fingerprint before sending credentials. Never implement a blanket “trust any certificate” switch. frpc forwards the encrypted HTTPS stream; frps does not replace the Host certificate. Anyone holding the credential has access to the Host's exposed operations; v1 does not implement user/device roles.

## Endpoints

| Endpoint | Purpose |
| --- | --- |
| `GET /v1/status` | Protocol/app version, runtime mode, readiness, actual Codex Home and binary, runtime error, event epoch/checkpoint, certificate fingerprint |
| `GET /v1/capabilities` | Runtime-supported RPC methods and Host transport features |
| `POST /v1/rpc` | Structured native Codex operation |
| `GET /v1/events?after=N&epoch=E` | Authenticated WSS stream; replays events after N |
| `GET /v1/threads/{id}/access` | Current mutation policy: `canMutate`, `hostCreated`, `sharedProcess`; not a thread existence check |
| `GET /v1/requests` | Current pending server approvals and input questions |
| `POST /v1/requests/{requestId}/respond` | Single-use structured reply to a pending request |
| `POST /v1/runtime/connect` | Reconnect a disconnected Codex transport; does not restart an existing daemon |
| `GET /v1/projects/{projectId}/files?path=relative&root=0` | Directory entries under an authoritative project root |
| `GET /v1/projects/{projectId}/file?path=relative&root=0` | UTF-8 file contents, up to 2 MiB |
| `GET /v1/projects/{projectId}/git?root=0` | Git porcelain status, staged/unstaged diffs, up to 4 MiB each |
| `GET /v1/tunnel` | frpc process state; not proof of internet reachability |
| `POST /v1/tunnel` | Validate and start bundled frpc with explicit configuration |
| `POST /v1/tunnel/stop` | Stop the Host-owned frpc process |

Directory results have `path` and `entries: [{name, kind}]`; `kind` is `file`, `directory`, or `symlink`. Absolute paths, `..` escapes and symlinks resolving outside the canonical project root are rejected. `.git` is omitted from directory listings. Git output does not include contents of untracked files. There is no arbitrary command execution API or filesystem write API.

## Native operations

Request:

```json
{"method":"thread/start","params":{"projectId":"project-id","cwd":"/absolute/host/project"}}
```

Response:

```json
{"result":{"thread":{"id":"native-codex-thread-id"}}}
```

Failures from Codex/Host policy use `{"error":{"code":-32601,"message":"...","data":{}}}`. `data` is optional; native result objects are preserved. HTTP body/envelope failures use HTTP errors. RPC failures have HTTP 200 and an error envelope; clients must check the envelope. Request body limit is 8 MiB. HTTP callers may run requests concurrently; Host assigns separate upstream correlation IDs.

The method names containing `/` are JSON protocol identifiers, not text entered into the conversation. The allowlist and installed runtime schema determine which identifiers are usable. The additive `threadAccess:true` capability advertises the thread access endpoint used by native clients to display read-only history and offer a fork. Fetch `/v1/capabilities` at connection time; do not infer support from app version alone.

Representative operations:

| Product action | Native method / fields |
| --- | --- |
| Projects | `project/list`, `project/read`, `project/create` (`idempotencyKey`, `name`, `roots:[{path}]`), `project/update`, `project/move` |
| List threads | `thread/list` with `projectId`, `cursor`, `limit`, `archived`, `searchTerm`, `sortKey` |
| Read history | `thread/read` for metadata; `thread/turns/list` (`itemsView:"full"`, `sortDirection`, `cursor`); `thread/items/list` for deeper pagination |
| Resume | `thread/resume` with original `threadId`, usually `excludeTurns:true` |
| Fork | `thread/fork` with `threadId` and `excludeTurns:true` |
| Send | `turn/start` with `threadId`, `input:[{type:"text",text:"..."}]` |
| Steer | `turn/steer` using the native active-turn precondition and typed input |
| Stop | `turn/interrupt` with `threadId`, `turnId` |
| Context | `thread/settings/update`, `thread/compact/start`, `thread/goal/*`, `thread/queue/*` when available |
| Organization | `thread/name/set`, `thread/archive`, `thread/unarchive`, `thread/metadata/update` |
| Capabilities | `model/list`, `permissionProfile/list`, `skills/list`, `plugin/list`, `mcpServerStatus/list` when available |

Native parameters and result schemas come from the installed Codex `app-server generate-json-schema --experimental`. `/v1/capabilities.methods` is the implemented Host allowlist intersected with those generated methods. Experimental fields can change between Codex versions; mobile adapters should decode only required fields and preserve unknown item/event types.

A default thread list explicitly includes CLI, VS Code, app-server and exec sources; Codex's default interactive-source filter alone omits some desktop threads. `nextCursor` must be retained and requested; do not treat a first page as the complete workspace.

Host never injects default model, cwd for an existing thread, weakened sandbox, or replacement instructions. New threads require a real absolute cwd. In an independent runtime, existing foreign thread mutations return `-32009`; read history or fork, or attach to the actual shared process. `thread/resume` cannot override the rollout path or entire history. Host-created ownership is persisted locally, not a global exclusive lease.

## Events and reconnection

A WebSocket text frame is:

```json
{"sequence":42,"epoch":"journal-uuid","payload":{"method":"item/agentMessage/delta","params":{"threadId":"...","turnId":"...","itemId":"...","delta":"text"}}}
```

Native notifications are retained unchanged in `payload`. In particular, only the matching `turn/completed` with `turn.status == "completed"` means success. A thread idle notification is not a substitute for completion. Track thread, turn and item IDs independently.

Host notifications: `host/runtime/connected`, `host/runtime/disconnected`, `host/request/pending`, `host/request/resolved`. Unknown methods should not disconnect the client.

Persist `(epoch, sequence)` after applying events. Reconnect with that cursor; skip sequences already applied. On a fresh connection, use a checkpoint from `/v1/status`, subscribe and buffer events, load project/thread/history snapshots, then reconcile buffered events using stable IDs. Avoid replaying old deltas on top of complete historical messages. `after=0` requests the retained window and should only be used for a fresh event journal view.

A positive cursor with a missing/wrong epoch, a future sequence, or a sequence older than retained history returns HTTP 409 before upgrade. A live slow consumer receives `{"error":"resync_required"}` and is disconnected. Fetch authoritative history and pending requests, establish a new checkpoint, and reconnect. Restarting Host preserves journal identity; replacing the state directory creates a new epoch. Native RPC transport disconnects are explicit; the phone must refresh status and can request reconnect.

The backend does **not** automatically retry mutations or implement universal request idempotency. A 60-second RPC timeout (`-32002`) means outcome unknown. Inspect current state before retrying; use native idempotency keys/client message IDs where available. Queues and goals are native capabilities, not a Host work scheduler.

## Approvals and questions

`GET /v1/requests` returns:

```json
[{"requestId":"opaque-host-uuid","request":{"id":"upstream-id","method":"item/commandExecution/requestApproval","params":{"threadId":"...","turnId":"..."}}}]
```

Reply to the Host UUID, never directly to an upstream request ID:

```json
{"result":{"decision":"accept"}}
```

Supported command/file decisions: `accept`, `acceptForSession`, `decline`, `cancel`. Typed question reply: `{"result":{"answers":{"question-id":{"answers":["chosen answer"]}}}}`. Permissions and MCP elicitation preserve their native structured response formats. Never turn arbitrary request strings into an automatic approval. Host validates the response shape before consuming it; the first accepted response wins. A repeated or expired request returns `-32004`. An acknowledgement means sent to the connected runtime, not that the tool execution succeeded.

Pending approvals are live transport state, retrievable after a phone reconnect, and cleared when the upstream disconnects. They are not resurrected after a backend restart. The event journal can contain a historical approval event that is no longer actionable; always reconcile with `/v1/requests`.

## Tunnel configuration

```json
{"server":"frps.example.com","serverPort":7000,"remotePort":9443,"token":"your-frps-token","trustedCaFile":null}
```

The frps token differs from the Host API token. Credentials are stored in a private generated config file, not command-line arguments. `trustedCaFile:null` uses system trust; a custom CA must be an existing absolute host path. The client verifies configuration with `frpc verify` before starting. Process states are `stopped`, `running`, `exited`, or `unknown`. Public reachability requires a separate check from the phone. No cloud service is provisioned by Host.
