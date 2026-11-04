# 网络接入

控制服务为 `https://connect.jintongshu.com`，由 Cloudflare Worker 承载，`workers.dev` 和预览入口关闭。relay 为 `https://relay.jintongshu.top/`，TCP 443 承载连接，UDP 7842 用于地址发现。

relay 集合由服务端 `NETWORK_POLICY` 通过签名证书下发，不写入二维码或设备授权。

## 应用验证

两端使用 Apple App Attest，服务验证证书链、一次性挑战、公钥、App ID、生产环境、断言计数器和签名扩展。团队固定为 `775Y7KV7KU`。

| 角色 | 应用标识 | 发行要求 |
| --- | --- | --- |
| 客户端 | `com.jintongshu.AnyWhereCoding` | TestFlight（2）或 App Store（4），签名构建号 ≥ 51 |
| Host | `com.jintongshu.AnywhereHost` | Developer ID（6），Apple 签名的 SHA-256 CodeDirectory 哈希匹配官方公证产物目录，构建号 ≥ 51 |

Mac 证明可不含构建号，未知代码哈希拒绝接入；额外检查 `aclBlob`，要求 SIP 与完整安全模式。不支持证明或验证失败时拒绝，不降级到共享密钥。系统要求见[分发](DISTRIBUTION.md)。

共享证明实现位于 `transport/apple/NetworkAdmission.swift`。Host 由签名外层 App 获取资格，经本机控制 socket 安装到后端。

## 接口与证书

接口均为 `POST /v3/network/` 下的 JSON 请求。

| 路径 | 字段与行为 |
| --- | --- |
| `challenges` | `keyId`、`role`（`host` / `client`）、`purpose`（`attest` / `lease`）；挑战 120 秒有效 |
| `attestations` | `keyId`、`role`、`challenge`、`attestation`；登记 Apple 公钥，不重置已有计数器 |
| `leases` | `keyId`、`role`、`challenge`、`data`、`assertion`；`data` 为 Base64 JSON，含 `endpointId`、端点 Ed25519 `signature` |
| `releases` | GitHub OIDC 授权；登记 `codeHash`、`build`、`version`、`sourceSha` |

端点签名原文按换行连接 `Anywhere-Network-Endpoint-v3`、控制服务源、App Attest key ID、角色、挑战、EndpointId。App Attest 断言签名原文为 `Anywhere-Network-v3\n<源>/v3/network/leases\n<挑战>\n` 后接解码后的 `data` 字节。

接入证书为 Ed25519 JWT，固定 `alg=EdDSA`、`kid=admission-1`、`aud=anywhere-network-v3`，包含 `iss`、端点 `sub`、`role`、`iat`、`exp`、随机 `jti` 和 `relays`。有效期 15 分钟，应用每 5 分钟续期。签发私钥仅存于 Worker Secret `NETWORK_SIGNING_KEY`；两端与 relay 固定验证公钥，证书绑定对应 iroh 私钥。

原生连接先通过 `admission.exchange` 双向验证证书，再执行[设备配对](SECURITY.md)；`admission.renew` 在原连接续期。直连和 relay 均校验签名、端点身份、角色与有效期；官方资格不授予 Host 操作权限。

relay 每秒清理过期连接，单端点最多 4 条连接，全局最多 4096 条，每条接收限速 10 MiB/s。

## 策略与撤销

Worker Secret `NETWORK_POLICY` 字段：

| 字段 | 用途 |
| --- | --- |
| `disabled` | 停止签发 |
| `revokedKeys` | 被撤销的 App Attest key ID 文本 SHA-256 |
| `relays` | 下发的 relay URL 集合 |
| `revokedHostCodeHashes` | 可选，撤销公证产物 |
| `hostBuilds` | 可选，受控初始化目录；元素含完整 SHA-256 `codeHash` 和 `build` |

续期重新检查撤销列表与发布目录；关闭服务或撤销应用后，已签发资格最迟 15 分钟失效，失败续期不延长旧证书。更换签名密钥须同步更新两端与 relay 的公钥及 `kid`。

## 发布登记

`Scripts/register-host.py` 使用 GitHub OIDC 短期身份；固定 GitHub 发行者、公钥源、官方仓库、所有者数字 ID、`main` 分支和发布工作流路径。登记令牌一次性使用，audience 为 `https://connect.jintongshu.com/v3/network/releases`，`sub` 为 `repo:Xingsandesu@55939146/AnywhereHost@1440923483:ref:refs/heads/main`。

发布目录保存在 Durable Object，最多 256 项；达到上限须移除停用产物。撤销列表同时约束初始化目录和自动登记目录。

## 部署

relay 配置、反向代理和 systemd 单元位于 `Deploy/relay`：

```sh
sudo Deploy/relay/install.sh
sudo systemctl enable --now iroh-relay
sudo journalctl -u iroh-relay -f
```

`relay.toml` 配置 TLS 证书；后端与指标仅监听回环。443 反向代理 `/relay` WebSocket 和 `/ping`，HTTP 后端提供 `/generate_204`。代理禁止记录接入凭据；证书续期后重载，`upstream-roots.pem` 使用服务器系统 CA bundle，保持证书验证启用。

Worker 部署前执行[通知服务检查](../CONTRIBUTING.md)，首次配置或轮换 Secret：

```sh
cd notification-worker
npx wrangler secret put NETWORK_SIGNING_KEY
npx wrangler secret put NETWORK_POLICY
npm run deploy
```

更换 relay 时先部署并验证新节点，再更新策略，旧节点至少保留一个资格周期。依赖版本见[架构](ARCHITECTURE.md)，隔离测试与实机检查见[贡献指南](../CONTRIBUTING.md)。
