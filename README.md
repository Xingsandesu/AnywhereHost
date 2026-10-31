# Anywhere Host

macOS Codex 主机应用：**Rust 后端 + SwiftUI 控制台 + 内置 frpc**，交付一个 `AnywhereHost.app`，为 iPhone Duo 的远程开发体验提供结构化 API。

## 产品边界

- iPhone Duo 是工作界面，Mac 是执行主机；会话、项目、上下文和执行结果以主机 Codex 为准。
- Codex App 优先，独立 CLI 次之。复用原有 `CODEX_HOME`、登录、配置、技能、工作目录和线程 ID；不复制一个新的 Codex 用户环境。
- 创建项目、恢复会话、选模型、压缩上下文、审批和停止都是协议操作，不依赖 `/model`、`/stop` 或对提示词做命令解析。

**0.1 是可运行的 Host 基础版本，不代表 iPhone 和桌面已完全对齐。** iPhone 客户端通过 Host v1 支持多电脑、多工作区、原生会话/模型/上下文/审批及文件界面。详见旁边客户端工程的 `Docs/HostClient.md`。

## 运行

```sh
./Scripts/build-app.sh
open dist/AnywhereHost.app
```

需要 macOS 14+、本机已有 Codex App 或 CLI。源码构建使用 Rust 1.99+、Swift 6+ 和 Xcode 命令行工具。打包脚本从官方 release 下载固定版本 frpc 0.71.1，校验归档 SHA-256，把许可证与二进制一起放进 `.app`。不打包或复制 Codex。默认构建当前 Mac 架构；目前验证的是 Apple Silicon。

仅通过独立 `.app` 直接分发，放弃 Mac App Store / TestFlight。应用、Rust 后端和 frpc 均不启用 App Sandbox；不需要授权主目录、项目或 Homebrew 文件夹，Codex 执行权限和审批按其原生配置处理。macOS 文件权限与隐私控制仍然生效，需要访问受保护数据时可从应用打开完全磁盘访问权限设置。

本地构建使用 ad-hoc 签名和 Hardened Runtime。`ANYWHERE_SIGNING_IDENTITY` 可指定 Developer ID Application 签名身份，公证和自动更新尚未接入。详见[直接分发与打包](Docs/DISTRIBUTION.md)。

启动后可选择主机项目、读取会话历史、创建新会话、发送/停止、分叉和压缩上下文。菜单栏提供重新打开控制台和退出。退出 App 会停止其 Rust 后端和 frpc；关闭窗口不会停止服务。

“连接 iPhone”配置自有 frps 服务器。默认监听 `127.0.0.1:9443`，frpc 只转发该 HTTPS 端口。控制端口填写 frps 的 `serverPort`；Host 映射端口另选一个服务器允许且未占用的端口，两者不能相同。默认使用 frp 标准 TLS 加密（不验证 frps 证书）；填写 CA 文件路径可验证 frps 身份。Host 的 HTTPS 证书固定验证独立生效。设置会恢复上次配置，同一 frps 地址和控制端口可留空 token 使用已保存凭证；窗口显示连接诊断并可打开隧道日志。配对链接携带地址、访问令牌和 TLS 叶证书 SHA-256 指纹。它是持有即授权的个人设备连接凭证，当前没有逐设备撤销与一次性配对码。

## Codex App 优先的实际含义

1. 优先连接显式 `controlSocket` 或 Codex Home 中已有的标准 app-server 控制 socket。
2. 没有 socket 时，优先使用已安装 Codex/ChatGPT App 内置的 Codex 可执行文件，启动 Host 管理的 app-server。
3. 没有 App 运行时才查找 PATH/Homebrew 的 Codex CLI。

这台 Mac 上的 ChatGPT.app 内置 Codex 0.162.0-alpha.2 使用私有 stdio，没有标准控制 socket。因此实测模式是 **`appRuntime`：共享原来的数据和配置，独立执行进程**。不是实时接管正在运行的桌面进程。原桌面会话允许读取或分叉，直接恢复/修改返回 `-32009`；Host 新建/分叉的会话可以继续。不要同时在独立 Host 和桌面运行同一个 Host 创建的会话；创建来源登记不是跨进程互斥锁。

`sharedCodexProcess` 表示连接了外部共享 socket；它可能属于独立 daemon，不能仅凭 socket 存在就宣称连接了桌面 UI。桌面内专有插件、浏览器、工作树 UI 状态也不能通过启动 App 内置二进制自动获得。详见 [架构与后续计划](Docs/ARCHITECTURE.md)。

## API

[Host v1 接入协议](Docs/API.md) · [验证记录](Docs/VERIFICATION.md)

已实现：TLS + Bearer 认证、能力发现、结构化 RPC、流式事件及 SQLite 游标重放、待处理审批/输入请求、项目目录/文本/Git diff、隧道管理。实际 RPC 支持集合由安装的 Codex 导出 schema 与 Host allowlist 取交集；不伪造缺失能力。

## 配置和数据

`~/Library/Application Support/AnywhereHost/`：目录权限 0700，凭证及私钥 0600。包含配置、随机访问令牌、TLS 身份、事件数据库、主机日志和 frpc 配置。凭证使用受限文件存储。App 与 CLI 统一使用此默认目录，不读取旧容器目录，不提供兼容或迁移流程。

`config.json` 可覆盖 `listen`、`codexHome`、`codexBinary`、`controlSocket`、`frpcBinary`，以及可选的 `apns` 签名配置。改配置后重启 App。不直接编辑 Codex 数据库或 `.codex-global-state.json`。启动进程通过固定的登录 shell 环境探针恢复 PATH 等环境变量，再显式保留选定 Codex Home；这不是接受远端 shell 指令的 API。

Live Activity 后台更新使用真实 Apple APNs，需配置本团队的 APNs Key 并启用 iOS Push Notifications 能力。配置脚本默认将凭证信息保存到状态目录的独立 `apns.json`，可在升级旧版 Host 前准备；新版启动时加载。[配置、协议与当前验证限制](Docs/APNS.md)。

## 验证

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo build
python3 Scripts/verify-host.py
python3 Scripts/smoke-real-codex.py
python3 Scripts/verify-frp.py
python3 Scripts/verify-shared-codex.py
swift build
python3 Scripts/verify-app.py dist/AnywhereHost.app --integration
```

真实模型闭环测试需显式加 `--inference`：`python3 Scripts/smoke-real-codex.py --inference`。它调用现有主机登录，在临时目录中创建 ephemeral 会话，要求模型只回复标记文本，不调用工具；会产生少量模型用量。

## 远程桌面

控制台工具栏“远程桌面”可允许已配对的 iPhone 通过 App 内嵌 noVNC 控制此 Mac。首次需在系统设置开启“屏幕共享”并配置允许访问的用户；开关默认关闭。桌面复用现有 frp HTTPS/WSS 隧道，公网不需要新增 5900 端口。客户端可将屏幕共享凭证存入其钥匙串，后续一键连接。

Host 必须保持运行且 Mac 开机联网。当前没有远程唤醒、音频或文件传输。协议及边界见 [API](Docs/API.md)；客户端使用说明见相邻工程 `AnyWhereCoding/Docs/REMOTE-DESKTOP.md`。

`python3 Scripts/verify-desktop.py` 在未占用的本机 5900 上创建隔离字节服务验证认证、WSS 转发与撤销，不更改系统屏幕共享设置。真实 Mac 登录和蜂窝网络连接需另行验收。
