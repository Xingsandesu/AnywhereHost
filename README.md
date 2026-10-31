# Anywhere Host

多设备远程 Codex 主机。SwiftUI 主机管理界面、Rust 后端和固定校验版本的 frpc 打包为一个 `AnywhereHost.app`；使用本机已安装的 Codex，不分发 Codex。

## 构建与运行

```sh
./Scripts/build-app.sh
open dist/AnywhereHost.app
```

运行需要 macOS 14+ 和已登录的 Codex App 或 CLI。源码使用 Xcode 27 / Swift 6、Rust 1.99+ 构建。macOS 26+ 使用原生 Liquid Glass；较早系统使用标准材质。当前打包目标为 Apple Silicon，采用 Hardened Runtime 和本地 ad-hoc 签名，未接入公证或自动更新。

Mac 应用仅提供服务状态、设备连接、隧道、远程桌面、权限和主机启停。项目、会话、模型与审批操作由远程客户端通过 Host 协议完成。Mac 管理界面不加载会话历史或订阅消息流。关闭窗口保留服务，退出应用停止其子进程。

## 连接设备

“连接设备”配置自有 frps 的地址、控制端口、映射端口和令牌。控制端口与映射端口必须不同；默认转发本机 `127.0.0.1:9443` 的 HTTPS 服务。CA 留空使用 frp 默认加密，填写 CA 可验证 frps 身份。Host HTTPS 独立使用证书指纹验证。

二维码与链接携带地址、五分钟单设备配对票据和证书指纹，供支持安全协议 v2 的客户端导入。设备注册后使用独立密钥签名，Mac 可单独撤销设备。更新后需重新扫码；隧道进程运行不代表公网已可达。

远程桌面默认关闭，在系统设置开启屏幕共享后，可允许已配对客户端通过现有 HTTPS/WSS 隧道连接。屏幕共享仍需独立身份验证；没有远程唤醒、音频或文件传输。

## 运行时与数据

依次使用已有控制 socket、已安装 App 内置运行时、CLI。保留原 `CODEX_HOME`、登录、配置、项目和会话 ID。独立启动的 App 运行时共享磁盘数据，不控制正在运行的桌面进程；外部会话可读取或分叉，不能直接修改。实际支持的方法由本机 schema 与 Host allowlist 共同决定。

状态目录为 `~/Library/Application Support/AnywhereHost`，存放配置、凭证、TLS 身份、事件和日志。`config.json` 可指定监听地址、Codex 路径、控制 socket 和 frpc；APNs 可使用独立 `apns.json`。目录权限为 0700，凭证和私钥为 0600。应用不启用 App Sandbox，macOS 隐私控制和 Codex 原生审批仍然生效。

## 文档与验证

- [协议](Docs/API.md)
- [架构](Docs/ARCHITECTURE.md)
- [打包与分发](Docs/DISTRIBUTION.md)
- [验证](Docs/VERIFICATION.md)
- [流式同步](Docs/SYNC-PERFORMANCE.md)
- [APNs](Docs/APNS.md)

安全协议已硬切至 v2：一次性扫码配对、独立设备签名、TLS 1.3、自签证书指纹验证与设备撤销。需同时更新客户端并重新扫码；没有旧 Bearer 回退。[安全设计与验证](Docs/SECURITY-V2.md)
