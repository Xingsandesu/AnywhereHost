# 实时活动通知

服务源见[网络接入](RELAY.md)，APNs 主题固定为 `com.jintongshu.AnyWhereCoding.push-type.liveactivity`。仅向官方客户端授权的活动发送阶段、计数与时间，不包含对话正文、项目路径或命令。通知经 HTTPS 交给 Worker 与 APNs 处理。

客户端使用生产 App Attest，应用身份与验证项见[应用验证](RELAY.md#应用验证)；通知接口最低签名构建号为 1。证明不可用时拒绝推送。Host 使用独立 P-256 私钥 `notification-key-v2.pk8`，权限 `0600`，私钥不离开 Host。

`config.json` 的 `notifications` 省略或为 `null` 时自动启用；`{"notifications":{"enabled":false}}` 关闭。

## 授权流程

1. 客户端向 `POST /v2/challenges` 提交 `keyId`、`purpose`（`attest` / `subscribe` / `revoke`），取得 120 秒有效挑战。首次通过 `/v2/attestations` 提交 `keyId`、`challenge`、`attestation`，登记公钥和计数器。
2. 客户端通过[原生 RPC](API.md) `activity.grant` 提交 `registration`、`keyId`、`tokenDigest`。`registration` 含 `activityId`、空 `deliveryToken`、`environment`、`threadId`、`turnId`、Unix `startedAt`、递增 `tokenVersion`；`tokenDigest` 为小写十六进制 Apple Token 文本的 SHA-256。
3. Host 返回 `url`、`grant`。签名绑定服务源、Host 公钥、客户端 key ID、设备与任务上下文摘要、环境、Host EndpointId、活动 ID、令牌版本和 Token 摘要；实际任务 ID 不上传。
4. 客户端核对服务源与已配对 Host 身份，将 `grant`、`activityId`、`pushToken`、`tokenVersion` 编码为 Base64 JSON `data`，向 `/v2/subscriptions` 提交 `keyId`、`challenge`、`data`、`assertion`。断言签名原文为 `Anywhere-Relay-v2\n<完整操作URL>\n<challenge>\n` 后接解码后的 `data`。只接受 `production`。
5. 服务返回 `deliveryToken`（128 个十六进制字符，不是 Bearer）。客户端填入原 `registration`，调用 `activity.register`；Host 经 `/v2/validate` 验证后入队。
6. 客户端通过带断言的 `/v2/revoke` 提交 `{"activityId":"…"}`；Host 通过签名的 `/v2/retire` 撤销订阅。失败进入持久重试队列，8 小时墓碑阻止晚到注册恢复活动。

活动最长 8 小时，Token 轮换不延长到期；版本回退、同版本换 Token、跨 Host 使用和结束后更新均拒绝。每客户端最多 256 个活动、16 台有有效活动的 Host；授权结束后释放名额。撤销阻止后续投递，已交给 APNs 的请求无法撤回。

## Host 签名与投递

`/v2/validate`、`/v2/deliver`、`/v2/retire` 使用固定 RFC 9421 profile：

```text
Signature-Input: host=("@method" "@target-uri" "content-digest");created=<Unix秒>;expires=<created+90>;nonce="<32位随机小写十六进制>";keyid="<SEC1公钥SHA-256>";alg="ecdsa-p256-sha256";tag="anywhere-relay-v2"
Content-Digest: sha-256=:<原始正文SHA-256的标准Base64>:
Signature: host=:<固定64字节r||s签名的标准Base64>:
```

签名依次覆盖方法、完整 URL、Content-Digest、Signature 参数。接受过去 90 秒、未来最多 30 秒；nonce 保留至过期后 31 秒。重试重新签名；禁止算法协商、查询串、浏览器 Origin 和 Authorization。

`/v2/deliver` 接收 `deliveryToken`、`payload`；`payload.aps` 仅允许 `timestamp`、`event`、`content-state`。Worker 固定接收者、环境、主题和消息格式。仅结束事件与首次进入审批使用优先级 `10`，其他使用 `5`；更新 `stale-date` 为消息时间加 180 秒，结束 `dismissal-date` 为消息时间加 60 秒。

Host 队列最多 256 个活动，每个活动仅一项在途投递。完成和审批变化立即排队，普通阶段变化每 5 秒合并、同阶段进度每 15 秒合并、未变化活动每 60 秒心跳。APNs 超时 15 秒，拒绝重定向；返回 200 只表示 APNs 接受。

重试遵守 `Retry-After` / `retryAfter`，使用 5 秒至 5 分钟退避。`retryScope:subscription` 仅暂停相关订阅；网络故障、服务级限流与未标记作用域的 5xx 使用共享退避。负载拒绝暂停订阅 5 分钟，`410` 删除无效订阅。

## 状态与限额

授权、挑战、计数、nonce、活动和额度由单个 SQLite Durable Object 协调。挑战消费、计数递增和额度预留使用事务；重启、令牌轮换与活动撤销不得重置防滥用额度。已调用 APNs 的失败或超时计为一次尝试，不退额度。

| 范围 | 限额 |
| --- | --- |
| 入口 IP | 120 / 分钟软限流 |
| 全局有状态请求 | 6000 / 分钟 |
| 挑战 | 全局 600 / 分钟，每个 key ID 12 / 分钟 |
| 证明尝试 | 全局 60 / 分钟，每个来源 IP 6 / 分钟 |
| 注册身份 | 1000 / UTC 日，最多 100000 个身份 |
| 单客户端订阅注册 / 撤销 | 30 / 60 次每分钟 |
| Host 请求 | 600 / 分钟 |
| APNs 投递 | 全局 6000 / 分钟、100000 / 日；每客户端 600 / 分钟；每活动 20 / 分钟 |
| 请求体 | 8 KiB；首次证明 32 KiB；APNs 负载 4 KiB |

服务保存 Apple Token 与授权至到期清理；证明公钥、计数、收据和防滥用身份持续保存。存储使用 Cloudflare 加密，应用日志关闭；平台仍处理连接 IP。`/v2/status` 返回配置状态，不授予推送权限。

## 部署与封禁

执行[通知服务检查](../CONTRIBUTING.md)后部署：

```sh
cd notification-worker
npm run deploy
cd ..
python3 Scripts/verify-notifications.py --remote
```

Wrangler 使用已登录账户，Durable Object 迁移随部署执行。APNs 私钥只存 Worker Secret `APNS_CONFIG`，轮换命令：

```sh
python3 Scripts/notifications.py upload-apns --team-id 775Y7KV7KU \
  --production-key-id KEYID --production-key /secure/production.p8
```

Worker Secret `NOTIFICATION_POLICY`：

```json
{"disabled":false,"revokedDevices":[],"revokedHosts":[]}
```

`revokedDevices` 为 App Attest key ID 文本 SHA-256，`revokedHosts` 为 SEC1 公钥字节 SHA-256。`disabled:true` 停止后续投递，撤销接口仍可用；无效策略拒绝投递。使用 `npx wrangler secret put NOTIFICATION_POLICY` 从标准输入更新。
