# 架构

Host 本机界面管理服务、连接、权限与生命周期；会话、项目、模型和审批由客户端承担。

## 传输

`transport` 是两端共用的 Rust 库，提供原生 Endpoint、Connection、流及 iOS C ABI。iOS 通过 Git 子模块固定源码提交，构建见[分发](DISTRIBUTION.md)。

`Vendor/iroh` 固定 iroh 1.4.1，上游提交 `d6c6963b9602d1b8dd2ba53b6519e7eb19cc7940`；本地补丁在每次 relay 重连时读取最新接入凭据。relay 服务使用同版本 `iroh-relay`。

iroh 负责地址发现、打洞、路径选择和迁移；业务层只在连接失效后重连。[网络接入](RELAY.md)下发 relay 集合，[原生协议](API.md)定义 RPC、事件、资源与桌面流。

## 运行时

接入顺序：已有 Codex 控制 socket → 已安装 App 运行时 → CLI。共享接入先验证 socket 对端用户与 PID，从实际对端可执行文件生成 schema，并核对初始化返回的运行时版本；无法确认来源或版本不一致时明确拒绝。独立进程才使用配置或自动发现的 App / CLI。Host 白名单与运行时 schema 的交集决定可用方法。保留实际 Codex Home、项目和会话 ID、cwd 与配置。独立 app-server 共享磁盘数据，不代表附着桌面进程；非共享进程不得修改非 Host 创建的会话，分叉通过原生方法执行。

## 状态

状态位于 `~/Library/Application Support/AnywhereHost`。目录权限为 0700，身份和授权配置文件为 0600，文件锁限制每个状态目录仅运行一个后端。

`identity.key` 保存稳定 Ed25519 身份；`devices.json` 保存设备授权；`events.sqlite` 保存事件；`operations.sqlite` 保存修改请求的派发状态与结果。本机管理通过 0600 Unix socket `control.sock`。

`config.json` 接受 `codexHome`、`codexBinary`、`controlSocket`、`notifications` 和 `relayUrls`；未知字段拒绝启动。`relayUrls` 在启动时清空，使用接入证书下发的地址；通知开关见[通知服务](APNS.md)。关闭窗口保留服务，退出应用停止自有进程。
