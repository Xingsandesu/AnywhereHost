# 开发指南

Host 是独立仓库，不依赖客户端源码。修改前请阅读 [工程约束](AGENTS.md) 和 [架构](Docs/ARCHITECTURE.md)。

## 本地检查

```sh
cargo fmt --all -- --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked
python3 Scripts/verify-host.py
python3 Scripts/verify-security.py
swift build
swift test
python3 Scripts/version.py
```

Python 需支持 TLS 1.3（建议 Python 3.11+，勿使用旧版系统 LibreSSL Python）。协议测试使用临时配置、假 Codex 和 loopback；不修改真实会话。桌面单元测试使用随机端口，包含空闲心跳及 4 MiB 双向压力回归。`verify-desktop.py` 另需空闲的 5900 端口，不能与正在运行的系统屏幕共享同时使用。

仅 `Scripts/smoke-real-codex.py --inference` 会执行实际推理，默认不运行。不要把 fixture 通过描述为真实手机 / 公网 / Mac 屏幕共享全部通过。

## 版本与发布

`python3 Scripts/version.py --set 0.1.5 --build 8` 同时更新 Rust 和 Xcode 版本，然后运行 `cargo check` 更新 lockfile。版本显示读取打包的 Info.plist；后端 `/v1/status` 与 `--version` 使用 Cargo 版本。发布之前必须通过一致性检查。

[分发文档](Docs/DISTRIBUTION.md) 描述打包、签名、GitHub Release 和 OTA。私钥保留在 macOS Keychain，不能放进仓库、构建日志或 Release。构建产物位于已忽略的 `dist/`。

## Debian 13 development

See [Linux build and tests](Docs/LINUX.md). Linux UI sources are in `Linux/anywhere_host`; use system Python with GTK 4/PyGObject. `PYTHONPATH=Linux python3 -m anywhere_host` runs against `target/debug/anywhere-host`; `--backend` and `--state-dir` support isolated development. Run the existing Rust checks plus Linux management, GTK and extracted-deb checks. macOS packaging and Swift remain separate platform checks.
