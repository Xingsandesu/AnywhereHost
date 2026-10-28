# AnywhereHost engineering constraints

- This is a separate project from AnyWhereCoding. Do not introduce cc-connect as a runtime, build, data or protocol dependency.
- Product target: full native iPhone Duo remote Codex experience, with this Mac providing structured APIs and real host state.
- Prefer installed Codex App/shared process integration; installed CLI is fallback. Preserve actual Codex Home, project/thread IDs, cwd and configuration. Never claim a new app-server process controls the current desktop process.
- No slash-command parsing, prompt-generated control actions, or shell command strings from remote clients. Use protocol methods and explicit resource APIs.
- Keep source modular; ship one .app with the Rust backend and checksum-pinned frpc. Do not redistribute Codex itself.
- UI capabilities must follow actual runtime support. Report unsupported operations explicitly; do not synthesize successful state or fake repository results.
- Run cargo test, cargo clippy, Scripts/verify-host.py and swift build for relevant changes. Read-only live Codex smoke is separate from the opt-in --inference check. Do not mutate existing user threads in tests.
- Keep docs precise about implemented RPCs, implemented UI, tested behavior and remaining work. Do not equate a scaffold with complete desktop parity.
