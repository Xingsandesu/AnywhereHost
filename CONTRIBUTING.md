# 贡献指南

## 检查

```sh
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --locked --all-targets -- -D warnings
cargo build --locked --bins
python3 Scripts/verify-host.py
python3 Scripts/version.py
cargo audit
swift build
swift test
```

通知服务：

```sh
cd notification-worker
npm ci
npm test
npm run check
```

## 隔离与实机验证

所有测试使用临时状态，不得修改已有用户会话。原生测试强制经过临时 relay，覆盖配对、重连、事件恢复、去重与撤销。

debug 后端的 `--test-relay http://127.0.0.1:端口` 仅允许回环，不写入配置；release 拒绝此参数。客户端隔离测试使用 `ANYWHERE_NETWORK_TEST`，生产构建不定义该标志。

有客户端源码检出时，设置 `ANYWHERE_CLIENT_SOURCE` 后执行：

```sh
python3 Scripts/verify-native-client.py --client-root "$ANYWHERE_CLIENT_SOURCE"
python3 Scripts/version.py --client-root "$ANYWHERE_CLIENT_SOURCE"
```

| 检查 | 命令与要求 |
| --- | --- |
| 真实 Codex 只读检查 | `python3 Scripts/smoke-real-codex.py`；推理须显式启用且仅使用新建会话 |
| relay 接入与过期 | `cargo run --locked -p anywhere-relay --example probe -- /tmp/probe.json`；仅使用临时端点 |
| 真实 Host App Attest | `AnywhereHost.app/Contents/MacOS/AnywhereHost --verify-network-admission`；正式签名包、普通用户图形会话，仅使用临时端点 |
| 通知配置与拒绝路径 | `python3 Scripts/verify-notifications.py --remote` |

relay 探针 JSON 字段为 `secret` 字节数组、`token`、`url`、`expected`（`allowed` / `denied`），可选 `waitForExpiry`、`renewToken`。

App Attest、网络切换、后台恢复、屏幕授权与 APNs 使用正式签名实机和专用测试活动验证；线上探针不替代真机验收。

版本变更集中写入 [CHANGELOG](CHANGELOG.md)；不得提交凭据、用户状态、测试报告或构建产物。[构建与分发](Docs/DISTRIBUTION.md)
