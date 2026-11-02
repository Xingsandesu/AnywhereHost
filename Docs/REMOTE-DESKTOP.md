# 远程桌面与断线排查

Host 在已认证的 `/v1/desktop/socket` 上转发 RFB 字节，在 macOS 只连接 `127.0.0.1:5900`；Linux 只连接 Host 私有目录中的 TigerVNC Unix socket，不接受远端指定目标。必须同时开启本机屏幕共享与 Host 的桌面开关。

## 转发行为

- 最多两条连接；最大客户端消息 1 MiB，Host 画面块 16 KiB。
- 双向异步转发，TCP_NODELAY 保证细小输入及时送出，不丢帧或重排 RFB 字节。
- 任一方向写入连续阻塞 30 秒关闭；90 秒未收到客户端数据或心跳关闭。
- 每秒检查设备撤销与本地开关。撤销无需等到下一次画面更新。
- 有活动会话时状态查询复用活动连接状态，不再每五秒对系统屏幕共享建立并立即关闭探测连接。

## 日志

主界面“更多 → 打开日志”。每条会话有随机 session ID，关闭记录 reason、duration_ms、uploaded、downloaded。不记录密码、画面或配对身份。

| reason | 含义 |
| --- | --- |
| `client_close` / `client_eof` / `client_read_error` | 客户端或传输链路关闭 |
| `client_write_timeout` / `client_write_error` | 无法继续把画面送到客户端 |
| `screen_eof` / `screen_read_error` / `screen_write_error` | macOS 屏幕共享端关闭或读写失败 |
| `screen_write_timeout` | 屏幕共享持续不接收输入 |
| `client_idle_timeout` | 客户端 90 秒无消息或心跳 |
| `device_revoked` / `desktop_disabled` | 本机主动撤销 |
| `invalid_client_frame` | 非二进制 RFB 数据 |

## 2026-10-31 调查

用户报告手机提示“连接断开”，Host 与系统屏幕共享进程仍运行。系统日志显示多次认证成功后约 15 秒断开，未发现对应的 Host 原始崩溃。现有客户端在等待 WebKit 消费画面期间暂停读取 WSS，心跳 pong 与画面共享有序传输；若画面积压超过 10 秒，旧心跳会误判连接死亡。

客户端已单独增加基于接收进度的心跳：只保留一个待确认 ping，只有 pong 和数据均停止进展 30 秒才判超时。客户端不在本仓库发布。此机制已加入确定性回归；是否解释用户的全部断开仍需实际手机链路验证。

Rust 回归覆盖空闲 pong、4 MiB 往返、撤销和完整字节序列。真实本机 TLS + URLSession 心跳超过 25 秒通过；这不等于真实屏幕共享登录、拖动和公网慢网验收。

Debian 13 的 Wayland/X11 共享、密码和桌面门户授权见 [Linux 指南](LINUX.md)。
