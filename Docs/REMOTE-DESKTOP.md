# 远程显示

配对设备通过 `desktop.native.open` 原生双向流访问远程显示。Host 本机开关必须开启；屏幕录制和辅助功能权限由 macOS 管理。每台 Host 最多两个会话，关闭共享、撤销设备或流断开时终止会话并释放输入。

Rust 后端只启动本机 GUI 进程通过 `ANYWHERE_DESKTOP_EXECUTABLE` 指定的绝对路径，固定参数 `--desktop-session`；远端不能选择可执行文件或传入 shell 命令。该进程通过标准输入输出交换有界消息，不监听额外网络端口。无需启用 macOS 屏幕共享或另存桌面登录密码。

画面由 ScreenCaptureKit 采集、VideoToolbox 硬件编码为 H.264，目标 60 Hz，禁用 B 帧。最多两帧等待客户端确认；拥塞时只保留最新待编码采集帧，不丢弃已编码的参考帧。帧率受网络、显示器更新与解码速度限制，不承诺固定帧率或延迟。

扩展模式动态绑定 CoreGraphics 的 `CGVirtualDisplay` 私有接口，创建会话专属 HiDPI 显示器，退出时释放。私有接口变化可能导致扩展显示不可用，届时报告具体错误。

## 消息

首次响应仍为传输层 `ready:true` 帧。此后的每条消息为大端 `UInt32` 长度、`UInt8` 类型、内容；长度包含类型字节，上限 8 MiB。输入 JSON 上限 16 KiB。

| 类型 | 方向 | 内容 |
| --- | --- | --- |
| 1 | Host → 客户端 | JSON：`version:1`、`codec:h264`、`width`、`height`、`generation`、Base64 SPS/PPS 数组 `parameters` |
| 2 | Host → 客户端 | AVCC 视频访问单元，NAL 长度字段为 4 字节 |
| 3 | 客户端 → Host | JSON `event`：`start`、`resize`、`ack`、`pointer`、`scroll`、`key`、`text`、`release`、`menu`、`dock` |
| 4 | Host → 客户端 | UTF-8 会话错误 |

`start` 指定 `mode:mirror/extend` 和像素 `width/height`。`resize` 更新像素 `width/height`，保持扩展显示器标识，释放按键后重启采集与编码。`ack` 回传对应配置的 `generation`，旧画面的确认不影响新编码器背压。视频上限 3840×2880；扩展模式优先放大至 1600×1200 的 HiDPI 范围，再在视频上限内保持宽高比。`pointer` 使用显示器内归一化坐标、`phase:down/move/up`、`button:0/1/2`，Pencil 另含 `kind:pencil`、`pressure`、`tiltX/tiltY`、`eraser`。键盘 `code` 为 macOS 虚拟键码，`down` 为按下状态。所有事件限于当前会话的显示器与由会话持有的按键状态。
