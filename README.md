# AnywhereHost

把你的 Mac 或 Debian 主机变成随身可连接的 Codex 工作主机。macOS 使用 SwiftUI App；Debian 13 amd64 使用 GTK 4 界面与 deb 包。两者复用 Rust 服务和 frp 隧道，提供安全设备配对及项目与会话协议；远程桌面在 macOS 使用系统屏幕共享，在 Linux 使用 Wayland / X11 本机共享。

**配套原生 iPhone 客户端 Anywhere 正在上架中。** 本仓库仅公开 Host，客户端独立开发，不包含在此仓库中。

[下载最新版](https://github.com/Xingsandesu/AnywhereHost/releases/latest) · [更新记录](CHANGELOG.md) · [构建与 OTA](Docs/DISTRIBUTION.md) · [安全设计](Docs/SECURITY-V2.md)

## Linux（Debian 13 amd64）

提供 GTK 管理界面、Rust 后端和固定校验和的 Linux frpc。源码构建：`python3 Scripts/build-deb.py`；安装：`sudo apt install ./dist/anywhere-host_0.1.4-1_amd64.deb`。从应用菜单或 `anywhere-host-gtk` 启动。依赖、权限、运行时配置与验证说明见 [Linux 指南](Docs/LINUX.md)。

## macOS 使用

需要 Apple Silicon Mac、macOS 14+，以及已经登录的 Codex App 或 CLI。Host 使用本机已安装的 Codex，不分发 Codex。

1. 下载 Release 中的 ZIP，解压并将 `AnywhereHost.app` 放入“应用程序”。当前发行包使用 Apple Development 签名，尚未完成 Developer ID 公证；首次启动可能需要在系统设置的“隐私与安全性”中允许打开。
2. 启动 Host，完成设置向导。主界面和菜单栏显示版本号，应用菜单提供“检查更新…”。
3. 在“连接设备”填写自有 frps 的地址、控制端口、映射端口和令牌。控制端口与映射端口须不同；默认转发本机 `127.0.0.1:9443` 的 HTTPS 服务。
4. 使用客户端扫描二维码，完成一次性设备配对。配对票据五分钟有效，每个设备独立签名，Mac 可撤销设备。
5. 使用远程桌面时，在 macOS 系统设置开启“屏幕共享”，再在 Host 中允许已配对设备连接。桌面复用原有 HTTPS/WSS 隧道，不需要暴露公网 5900 端口。

升级到 0.1.4 后支持 Sparkle OTA：自动检查、提示下载，由用户选择安装并重启；更新包经过 Ed25519 签名验证。旧版没有更新器，需要先手动安装一次。OTA 不会覆盖配对、证书或配置。安装重启会中断当时的远程连接。已启动的隧道会在后续 App 重启 / OTA 后自动恢复；主动停止隧道则不会自动重启。

## 功能与边界

- Host 管理服务状态、设备、连接、权限、隧道和生命周期；项目、会话、模型与审批由客户端操作。
- 原生协议支持流式事件、历史分页、文件读取、Git 状态、审批、任务和可选 APNs Live Activity 更新。
- TLS 1.3、证书指纹固定验证、一次性配对、设备请求签名与防重放；不支持旧 Bearer 认证。
- 远程桌面只连接本机屏幕共享，最多两个会话；屏幕共享仍需独立认证。不提供远程唤醒、音频或文件传输。
- 依次使用已有控制 socket、已安装 App 内置运行时、CLI。独立启动的运行时共享磁盘数据，不代表接管正在运行的 Codex 桌面进程。
- 关闭窗口保留服务；退出 App 停止其子进程。

macOS 状态保存在 `~/Library/Application Support/AnywhereHost`，Linux 使用 `$XDG_STATE_HOME/AnywhereHost`（默认 `~/.local/state/AnywhereHost`）。项目目录、登录、配置和会话 ID 保持原样。Host 不启用 App Sandbox，macOS 隐私控制和 Codex 原生审批仍然生效。请勿提交状态目录、密钥或配对二维码。

## 开发

源码使用 Xcode 27 / Swift 6、Rust 1.99+ 和 Python 3.11+。macOS 26+ 使用原生 Liquid Glass；较早系统使用标准材质。

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
swift test
./Scripts/build-app.sh
open dist/AnywhereHost.app
```

构建脚本会下载校验固定版本的 frpc 与 Sparkle；打包需要可用的 Apple Development 或 Developer ID Application 签名身份。完整说明见 [开发指南](CONTRIBUTING.md)。

| 目录 | 职责 |
| --- | --- |
| `Sources/AnywhereHost` | 原生管理界面、权限、后端进程和更新器 |
| `Linux` | GTK 4 管理界面、deb 元数据与隔离管理测试 |
| `backend/src` | Host API、设备安全、Codex、桌面转发、隧道与事件存储 |
| `Tests` | Swift 状态与权限回归 |
| `Scripts` | 构建、版本、发布及隔离协议验证 |
| `Resources` | Info.plist、权限声明和图标 |
| `Docs` | 架构、安全、协议、运维与验收记录 |

[协议](Docs/API.md) · [架构](Docs/ARCHITECTURE.md) · [桌面排错](Docs/REMOTE-DESKTOP.md) · [验证](Docs/VERIFICATION.md) · [流式同步](Docs/SYNC-PERFORMANCE.md) · [APNs](Docs/APNS.md)
