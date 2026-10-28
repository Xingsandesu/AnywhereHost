# macOS sandbox and Codex execution

The macOS application, Rust service and frpc are sandbox signed. The main
application permits incoming/outgoing network connections, persistent user-selected
read/write folders, and executable output. Both embedded services carry only the
App Sandbox and inheritance entitlements. Build and local packaging scripts retain
these entitlements when signing.

## First launch

The Host overview guides the user to **System Settings → Privacy & Security →
Full Disk Access** and can reveal the app in Finder so it can be added. macOS owns
this setting; the app does not claim it has been granted or set it programmatically.
Restart the app if macOS requests it.

Separately, select the actual user home directory in the macOS folder picker.
App Sandbox still needs that authorization even with Full Disk Access. The Host
stores security-scoped bookmarks and restores them before spawning any service.
Codex continues to use the real HOME, CODEX_HOME, login environment and histories;
Host credentials, certificates and logs live in its application container.

Homebrew at `/opt/homebrew` or `/usr/local`, external volumes and workspaces outside
the home directory need their own folder grants. Stop the Host after finishing
active work, use **Authorize development tools / other folders**, then start it.
New child processes inherit the updated scopes. Adding a project outside the saved
grants explains this requirement instead of promising that Codex can access it.
TCC-protected data and operations restricted independently by macOS can still fail.

## Execution policy

A real execution test found that nesting Codex's default Seatbelt sandbox inside
App Sandbox fails with `sandbox-exec: sandbox_apply: Operation not permitted`.
The signed native launcher identifies its sandbox, and the signed Rust helper uses
Codex's native `externalSandbox` policy, with `networkAccess: enabled`, for its
independently spawned runtime. The operating system continues to enforce the app's
sandbox and folder grants on tools. This policy is reported as
`runtime.executionSandbox = macOSAppSandbox` and displayed in the Host overview.
It applies to all authorized folders, not a separate per-project write boundary.

The backend confirms this policy through native `thread/settings/update` after
start/fork/resume and supplies it on `turn/start`. Original approval policy is
preserved. Explicit requests for a different sandbox mode, restricted network or
named permissions profile fail instead of being silently replaced. Unsandboxed
CLI hosts and attachment to an existing shared process retain native Codex policy.
Existing desktop thread mutation still requires a shared process or a new fork.

Reference: [Codex app-server sandbox policy](https://learn.chatgpt.com/docs/app-server)
and [Apple App Sandbox entitlements](https://developer.apple.com/library/archive/documentation/Miscellaneous/Reference/EntitlementKeyReference/Chapters/EnablingAppSandbox.html).

## Verification

`Scripts/SandboxVerification` is a development-only signed harness. It compiles the
production HostPermissions and HostProcess classes, embeds the built Rust/frpc
executables, and uses a separate application container, state directory and port
19443. The folder picker must receive a real user grant; the harness does not
manufacture permission tokens. It is not embedded in the shipped app.

```sh
mkdir -p /Users/Shared/AnywhereHost-SandboxVerification
printf OUTSIDE_GRANTED_HOME > /Users/Shared/AnywhereHost-SandboxVerification/read-probe
./Scripts/SandboxVerification/build.sh /path/to/AnywhereHost.app
open .build/SandboxVerification.app
python3 Scripts/SandboxVerification/verify.py
# Explicit opt-in to one ephemeral model/tool execution in a disposable workspace:
python3 Scripts/SandboxVerification/verify.py --inference
```

Quit/reopen the harness to test restoration without another picker. Do not run it
against production state or reuse existing threads. The shared-directory fixture
must remain outside the granted directories; no Full Disk Access assumption is
used to skip this boundary test.

Validated on 2026-10-28, arm64 macOS, installed Codex `0.162.0-alpha.2`:

- Main and inherited helper signatures include App Sandbox.
- Restored user-home bookmark survives application relaunch.
- Parent cannot read/write the ungranted shared-directory fixture; spawned child
  cannot read it.
- Installed Codex initializes with the original Codex Home and authentication
  configuration. Existing projects, threads and one history page can be read.
- A new ephemeral turn invokes the real command tool, writes `execution-proof.txt`,
  reads the expected marker, streams completion and returns exit code 0.
- Embedded sandboxed frpc forwards pinned Host HTTPS through a real loopback frps.
- Homebrew outside the home grant correctly reports an authorization error; grant
  its directory before using that toolchain. Built-in command execution succeeds.

The checks prove these paths, not arbitrary builds, every MCP integration, or full
access to every protected macOS resource. The verification harness requests no
Full Disk Access and never changes an existing user thread.
