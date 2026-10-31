# Live Activity APNs

Host 直接连接 Apple 的 HTTP/2 APNs 接口，更新和结束手机上已创建的 Live Activity。手机切到后台、锁屏或进程被系统回收后，Host 继续处理 Codex 原生事件并发送推送。Apple 接受请求不等于设备已显示；网络、电量、用户权限和系统预算仍影响送达。

## 配置真实 Apple 凭证

1. 在 Apple Developer 的 `com.jintongshu.AnyWhereCoding` App ID 启用 Push Notifications，更新主应用的开发及分发 provisioning profile。Live Activity topic 使用**主应用** bundle ID，不使用 `.Activity` 扩展的 bundle ID。
2. 为目标环境创建支持 APNs 的签名 Key，下载 `.p8`。App Store Connect API Key 不能代替 APNs Key。本项目 Team ID 为 `775Y7KV7KU`，实际 Key 必须属于同一开发团队。
3. App 与 CLI 默认使用 `~/Library/Application Support/AnywhereHost`。先启动 Host 生成 `config.json`，再将此目录传给配置脚本；CLI 显式使用 `--state-dir` 时传对应目录。
4. 在 Host 仓库执行以下命令，替换示例值。脚本复制本地密钥并保存独立的 `apns.json`，不向 Apple 发送任何内容，不打印密钥。旧版 Host 会忽略此文件，可在升级前预先准备凭证。

```sh
python3 Scripts/configure-apns.py \
  --state-dir '/absolute/path/to/AnywhereHost/state' \
  --environment production \
  --team-id 775Y7KV7KU \
  --key-id YOURKEY123 \
  --key-file '/absolute/path/to/AuthKey_YOURKEY123.p8'
```

TestFlight、App Store、Ad Hoc 使用 `production`；开发签名设备使用 `sandbox`。可分别运行命令配置两个环境；新 Apple Key 可以只对单一环境有效，不能把不匹配的 Key 填到另一环境。轮换已有配置需要显式 `--replace`。文件保存为 0600，配置中只存 ID 与文件路径。安装通过构建验证、包含 APNs 支持的新版 Host 后，启动时加载新 Key；不要在执行关键任务时重启独立 CLI runtime。

状态目录内 `apns.json` 的结构：

```json
{
  "teamId": "775Y7KV7KU",
  "bundleId": "com.jintongshu.AnyWhereCoding",
  "sandbox": null,
  "production": {
    "keyId": "YOURKEY123",
    "privateKeyPath": "/absolute/private/path/AuthKey_YOURKEY123.p8"
  }
}
```

默认不改动 `config.json`。也兼容将上述对象放入 `config.json` 的 `apns` 字段；已存在的内嵌配置优先，脚本更新它时保留其他字段。无配置、独立配置无效、密钥不可读或无效时，Host 核心连接功能继续工作，推送能力返回未启用，注册请求返回 503，不伪造注册成功。启动时仅检查密钥格式与配置；真实投递必须通过 Apple 请求及设备显示验证。

## 客户端与协议

- iOS `Activity.request(... pushType: .token)`；从 `pushTokenUpdates` 接收令牌并上传至原 Host。启动时也恢复监听，允许 ActivityKit 唤醒 App 后处理令牌轮换。旧版无 push token 的活动在确认 Host 支持推送后迁移一次，保留任务 ID 与开始时间。
- 读取 embedded provisioning profile 的 `aps-environment`，无 profile 的 Store 构建读取已签名的构建配置。Debug 为 development，Release 为 production；不在 400 响应后盲目切换 APNs 环境。
- 手机使用已有 HTTPS 证书固定验证和 bearer 鉴权。后台上传使用独立、短时 URLSession，不依赖前台 WebSocket，不使用永久后台运行。Keychain 凭证迁移至 AfterFirstUnlockThisDeviceOnly，支持锁屏时的令牌更新。
- `POST /v1/live-activities` 注册字段：`activityId`、十六进制 `pushToken`、`environment`（sandbox/production）、`threadId`、`turnId`、Unix 秒 `startedAt`、设备按活动持久化递增的 `tokenVersion`。必须对应 Host 观察到的原生任务；不接受客户端任意推送内容。
- `POST /v1/live-activities/remove` 接收 `activityId`、`pushToken`。删除只匹配当前令牌，短期 tombstone 防止晚到的注册请求恢复已取消活动。
- `GET /v1/live-activities/status` 返回已加载环境、订阅数、Apple 最近接受请求时间及脱敏错误；不返回 token、JWT 或私钥。需 Host v2 请求签名鉴权。
- `GET /v1/capabilities` 的 `liveActivityPush` 包含 `enabled`、`environments`。

## 投递与一致性

原生事件写 journal、任务状态投影与 outbox 更新使用同一 SQLite 事务；推送不依赖手机在线。仅保存阶段、时间、命令完成数量和内部去重 ID，不把代码、提示词、命令正文或回复内容放进 APNs。多手机、不同会话和 turn 独立订阅。

Host 使用 ES256 fixed-width `r || s` JWT，50 分钟内复用同一签名令牌，环境各自复用 HTTP/2 连接。TLS 使用 macOS `/etc/ssl/cert.pem` 公共 CA，并验证 Apple 主机名；不沿用配对 Host 的自签名策略。

首次投递和阶段变化使用 priority 10；思考、执行、回复等普通阶段变化按 5 秒窗口合并，审批、审批解除和终态跳过普通限速。仅完成数量等同阶段进展仍最多约每 15 秒一次，安静任务通过主机快照与约 60 秒心跳更新，这些更新使用 priority 5。持久化记录 Apple 已接受的阶段，升级保留已有订阅；发送途中发生的阶段变化也会重新安排，网络/429 等错误退避仍然有效。iOS 声明 `NSSupportsLiveActivitiesFrequentUpdates`，实际频繁更新权限与投递预算由系统和用户设置控制。`aps.timestamp` 按活动单调递增；`stale-date` 根据最后确认的任务状态计算，无法继续验证时不会把陈旧信息伪装成实时状态。终态发送 `event: end`，90 秒后从锁屏移除。

外层 APNs `timestamp`、`stale-date`、`dismissal-date` 使用 Unix 秒；`content-state` 中 Swift `Date` 使用默认 Codable 的 2001 年参考时间（Unix 秒减 978307200）。内容键名与 `AgentActivityAttributes.ContentState` 一致。

网络/429/5xx 指数退避并保留最新状态；403 降频等待配置修正或 token 刷新；410、BadDeviceToken、DeviceTokenNotForTopic 清除失效订阅；其他永久错误停发并暴露脱敏原因。旧 token 请求的晚到结果不影响新 token。订阅在创建后最多保留八小时，普通状态与取消记录也定期清理。

支持更新和结束**已有**活动；本次未实现 push-to-start、普通通知推送或一条活动跨多个 turn 自动重用。

## 验证与上线检查

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked
python3 Scripts/verify-host.py
python3 Scripts/test-apns-config.py
swift build
```

Rust 测试覆盖 ES256 验签/缓存、APNs 错误分类、无手机 socket 的状态生成、命令去重、审批恢复、终态防回退、跨轮次隔离、令牌轮换、延迟注册/删除、持久化恢复和重试。iOS `ActivityPushTests` 覆盖签名环境、注册身份和实际 APNs 内容的 Date 解码。

2026-10-29 16:07 验证：当前版本 23 项 Rust 测试、严格 Clippy、63 项隔离 HTTPS/WSS 检查及 Swift 构建通过。iOS 真机签名构建通过，实际签名包含 development APNs entitlement；模拟器 63 个用例中 61 通过、2 个额外网络环境用例跳过、0 失败。配置脚本回归与安装脚本 15 项回归通过。

production/sandbox 两把关联 APNs Key 已保存到桌面并通过 P-256 签名验签。主应用开发、App Store 和 Activity 扩展描述文件已安装。直接分发 Host 使用 `~/Library/Application Support/AnywhereHost`；已通过配对叶证书确认运行目录，并将两套凭证显式配置到其中，没有迁移旧会话数据。隔离 Host 使用真实密钥启动，确认两个环境均可加载。当前运行的 Host 尚未重启，仍报告 APNs 未启用；真实设备推送、锁屏显示及 TestFlight production 环境仍待验收。

上线前须完成：能力开通/签名验证 → 全部编译测试 → 安装新 Host 与 iOS App → 开始真实任务 → 锁屏观察阶段/审批/结束 → Host 重启恢复 → TestFlight production 环境复验。不要以单个 HTTP 200 代替设备显示验收。

依据：[Apple ActivityKit Push](https://developer.apple.com/documentation/activitykit/starting-and-updating-live-activities-with-activitykit-push-notifications)、[Token-based APNs](https://developer.apple.com/documentation/usernotifications/establishing-a-token-based-connection-to-apns)、[APNs HTTP requests](https://developer.apple.com/documentation/usernotifications/sending-notification-requests-to-apns)。

2026-10-31 调度修复验证：Rust 39 项测试、严格 Clippy、Rust/Swift 构建及 65 项隔离 HTTPS/WSS 集成检查通过。新增覆盖阶段切换限速与优先级、发送途中的审批解除及普通阶段变化、同阶段心跳、失败退避和原订阅数据库迁移。只读检查当前运行的 Host 已启用 sandbox/production，最近保留的完成状态已获 Apple 接受且无失败订阅。18:22（北京时间）已将通过打包验证的新版安装到 `/Applications/AnywhereHost.app` 并重启；Codex 连接正常，sandbox/production APNs 启用，配对令牌、证书及配置哈希保持一致。旧版备份与安装回执位于 `~/Library/Developer/Xcode/Archives/2026-10-31/`。配套 iOS 1.0 (20) 已上传并完成 Apple 处理（VALID），进入现有内部测试组，简体中文测试说明已保存验证；真实设备锁屏显示仍待验收。
