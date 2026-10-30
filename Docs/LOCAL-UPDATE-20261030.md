# Local Host update — 2026-10-30

Installed `/Applications/AnywhereHost.app` version **0.1.3 (6)** with the onboarding and permission checks. The previous app remains at `/Applications/.AnywhereHost-before-20261030-220044.app` for rollback.

The installed 0.1.2 service included newer attachment APIs whose source was not in this checkout. This local update retains that exact installed `anywhere-host` service and `frpc`, and packages the new `anywhere-permission-probe` separately. Rebuilding the repository alone does not reproduce the retained service; the installation manifest at `/tmp/host-local-update/manifest.json` records binary hashes. The source of that newer service still needs to be reconciled before a general release.

The Swift app includes the existing working-tree conversation settings changes and retains explicit MCP tool approval and supported primitive-form interactions. The permission helper and main application have Hardened Runtime signatures; the main app carries the Apple Events consent entitlement and usage description.

Validation completed:

- Release build and all four executable signatures verified.
- Seven Swift tests, Rust tests including the separate probe, and strict Clippy passed.
- The actual installation bundle passed 65 isolated HTTPS/WSS integration checks.
- Standalone directory probe performed real read/write/delete checks without leftover files.
- Before replacement: zero loaded Host threads and zero pending approvals.
- After replacement: Codex connected, tunnel running, attachment upload still advertised, runtime method set retained, APNs environments retained.
- Host configuration, pairing token/certificate/key and APNs configuration hashes unchanged.

No macOS privacy consent was automatically granted. The user can now open permission checks and complete the required system prompts from the updated Host.
