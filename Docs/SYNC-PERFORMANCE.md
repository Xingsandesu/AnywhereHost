# 同步性能与兼容协议

2026-10-28：与相邻 AnyWhereCoding 客户端的同步调度优化配套。

## HTTP 压缩

Router 使用 `tower-http` 的 `CompressionLayer`，仅启用 gzip。通过 `Accept-Encoding` 协商，不支持压缩的客户端保持原有 JSON；小响应及 WebSocket Upgrade 遵循库的默认谓词。认证、证书固定验证和 `cache-control: no-store` 不变。参考 [tower-http CompressionLayer](https://docs.rs/tower-http/latest/tower_http/compression/struct.CompressionLayer.html)。

压缩层不会缩小单个原生历史结果的逻辑大小；客户端首页减少到最近 5 轮，后续仍使用原生分页。超长单轮工具输出的按 item 懒加载尚未实现，不能声称有严格的历史字节上限。

## 可去重的文本增量

`GET /v1/capabilities` 的 `events.textDeltaOffsets` 为 `"utf8"`。

从原生 `item/started` 起，Host 为以下流维护已投递文本的 UTF-8 字节长度，在对应 delta 的 `params.hostTextOffset` 中给出本次增量的起点：

- `item/agentMessage/delta`：`item.text`。
- `item/plan/delta`：`item.text`。
- `item/commandExecution/outputDelta`：存在 command 时为 `command + "\n" + aggregatedOutput`，否则仅输出；空输出仍保留 command 后换行。
- `item/reasoning/summaryTextDelta`：额外提供 `hostSummaryIndex`，offset 对应该摘要分段的 UTF-8 字节长度；每个 item 最多跟踪 128 段。

位置按 `(threadId, turnId, itemId)` 隔离。未观察到起点的流不提供位置；缺失/越界的摘要分段索引不提供位置。item/turn 完成清理对应计数；超过 1,024 个未完成 item 时清空计数并退回无 offset 兼容行为，避免无限增长。标注发生在写入事件日志之前，所以重放保留相同位置与 sequence。

客户端可将 offset 与当前完整文本对比：已覆盖且相同的前缀跳过，只追加未覆盖部分。若 offset 超出当前文本或重叠内容不匹配，须补快照，不得盲目拼接。`item/completed` 是完整权威值。此机制不声称原生快照与事件日志具备跨进程原子事务。

旧客户端会忽略附加字段，继续使用原协议；新版客户端对旧 Host 保留受限频率的快照补偿。无需升级协议版本。

## 验证

- `streaming.rs`：UTF-8 位置、跨会话隔离、命令前缀、完成清理、缺失 started 不伪造位置。
- `Scripts/verify-host.py`：真实 HTTPS 的 gzip/identity 协商与正文一致性；WSS delta 位置在 journal 重放后保持不变。
- 相邻客户端 `HostSynchronizationTests.swift`：并发加载、请求合并、快照/事件重叠、Unicode、旧 Host、完成事件与取消行为。

2026-10-29 在正常开发环境完成标准回归：14 项 `cargo test`、`cargo clippy --all-targets -- -D warnings`、`cargo build`、44 项 `python3 Scripts/verify-host.py` HTTPS/WSS 集成检查及 `swift build` 全部通过；Cargo 已更新压缩依赖锁文件。相邻客户端的 46 项 iOS 测试全部通过（参数化展开共 50 次，0 跳过），包含双 Host HTTPS/WSS 与公网证书固定验证。尚未替换正在运行的 Host，不能将测试通过等同于生产性能收益已验证。
