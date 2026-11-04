# 原生协议

ALPN 为 `anywhere/0.4`。每个 RPC 或资源请求使用独立双向流；事件订阅使用持久双向流。JSON 帧为 4 字节大端长度及 UTF-8 JSON，每帧最多 20 MiB。请求为 `{"id":"UUID","method":"host.status","params":{}}`，错误为 `{"error":{"code":401,"message":"…"}}`。

| 方法 | 参数与行为 |
| --- | --- |
| `pairing.accept` | `ticket`；在已验证的对端身份上授权 |
| `host.status`、`host.capabilities` | 服务状态、运行时和可用方法 |
| `codex.call` | `method`、`params`；执行白名单内的原生 Codex RPC |
| `requests.list`、`requests.respond` | 待处理审批；响应使用 `requestId`、`result` |
| `thread.access` | `threadId`；查询修改权限 |
| `resource.files`、`resource.file`、`resource.image`、`resource.git` | `projectId`、`root`、`path`；根目录从真实项目读取 |
| `events.subscribe` | `epoch`、`after`；流持续输出事件或 heartbeat |
| `desktop.status`、`desktop.open` | 桌面状态；open 首次响应 `ready:true` 后传输原始 RFB |
| `runtime.connect` | 重新接入本机运行时 |
| `activity.grant`、`activity.register`、`activity.remove`、`activity.status` | 实时活动授权与订阅 |
| `pairing.create`、`devices.list`、`devices.revoke` | 仅限本机管理 socket |
| `admission.install` | 仅限本机；`token`，验证并安装 Host 接入证书 |

连接首先执行[接入资格交换](RELAY.md)，再按[设备授权](SECURITY.md)开放业务接口。

事件按 `epoch` 和 sequence 恢复；客户端只推进已应用事件的游标并丢弃重复序号。游标超出保留范围、Host 重启、运行时重新接入或事件写入失败时返回 `409`，客户端重新获取快照。事件写入恢复后更换 epoch；旧游标不能跨越未记录的事件。

修改请求的 `(设备身份,id)` 和内容摘要在派发前持久保留。同 ID 不同内容拒绝；有缓存的已完成请求返回原结果。响应正文保留最多 24 小时、1024 条和 32 MiB，超过限制只清理正文，防重放记录不删除，也不设置累计操作次数上限。未完成、结果过期或进程中断后结果未知时返回 `-32002`，相同 ID 永不重新派发。

修改操作由最多 64 个后台任务持有；60 秒响应超时或客户端断连不会取消已派发操作。迟到结果完成会话归属登记后写入操作日志；暂时写盘失败时保留结果并重试持久化。客户端连接失效后最多用同一个请求 ID 恢复一次；结果未知时要求用户核对状态，不得换 ID 自动重试。Host 或 Codex 进程中断不保证能够找回未返回的原生结果。读请求、无效方法及本机配对、撤销、接入资格安装和运行时重连不占用操作日志。

禁止命令字符串、斜杠命令控制、提示词控制动作和任意远端资源路径。资源读取、审批与运行时方法分别具有明确参数。

## 配对链接

`anywhere-host://pair` 查询字段为 `hostId`、`hostName`、`ticket`、`expires`（Unix 秒），均为必填，重复和未知字段拒绝。`hostName` 取系统电脑名称，客户端可修改显示名称。

## 连接状态

`devices.list` 返回 `devices`（已授权 EndpointId 数组）与 `connections`（以 EndpointId 为键的连接快照数组）。离线设备数组为空；同一设备的多个连接分别保留。

共享传输 C ABI 的 `status` 读取当前 `handle`，不发送网络请求。快照字段为 `connectionId`、`state`（`connected` / `disconnected`）、`mode`（`direct` / `relay` / `unknown`）、`rttMs`、`remoteAddress`、`error`。模式与往返延迟取自本端当前发送路径；未知路径或断开连接不返回旧延迟。
