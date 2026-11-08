# 远程显示

配对设备通过 `desktop.native.open` 原生双向流访问远程显示。Host 本机开关必须开启；屏幕录制和辅助功能权限由 macOS 管理。每台 Host 最多两个会话，关闭共享、撤销设备或流断开时终止会话并释放输入。

Rust 后端只启动本机 GUI 进程通过 `ANYWHERE_DESKTOP_EXECUTABLE` 指定的绝对路径，固定参数 `--desktop-session`；远端不能选择可执行文件或传入 shell 命令。该进程通过标准输入输出交换有界消息，不监听额外网络端口。无需启用 macOS 屏幕共享或另存桌面登录密码。

画面由 ScreenCaptureKit 采集、VideoToolbox 硬件编码为 H.264，目标 60 Hz，启用低延迟码率控制，禁用 B 帧与前瞻，跳过未完成的采集帧。采集和编码统一使用 sRGB 色彩原色／传递函数与 BT.709 YCbCr 矩阵。最多两帧等待客户端确认；拥塞时只保留最新待编码采集帧，不丢弃已编码的参考帧。帧率受网络、显示器更新与解码速度限制，不承诺固定帧率或延迟。

扩展模式动态绑定 CoreGraphics 的 `CGVirtualDisplay` 私有接口，创建会话专属 HiDPI 显示器，退出时释放。每次更新模式后，从 `CGDisplayCopyAllDisplayModes` 选择像素尺寸匹配且逻辑尺寸为其一半的模式，经 `CGDisplaySetDisplayMode` 应用并确认稳定，再启动采集，避免旋转时系统自动切到 1× 密度。私有接口变化可能导致扩展显示不可用，届时报告具体错误。

## 消息

首次响应仍为传输层 `ready:true` 帧。此后的每条消息为大端 `UInt32` 长度、`UInt8` 类型、内容；长度包含类型字节，上限 8 MiB。输入 JSON 上限 16 KiB。

| 类型 | 方向 | 内容 |
| --- | --- | --- |
| 1 | Host → 客户端 | JSON：`version:1`、`codec:h264`、`width`、`height`、`generation`、Base64 SPS/PPS 数组 `parameters` |
| 2 | Host → 客户端 | AVCC 视频访问单元，NAL 长度字段为 4 字节 |
| 3 | 客户端 → Host | JSON `event`：`start`、`resize`、`ack`、`pointer`、`scroll`、`key`、`text`、`touch`、`release`、`menu`、`dock` |
| 4 | Host → 客户端 | UTF-8 会话错误 |
| 5 | Host → 客户端 | JSON：`touchProtocol:hid-digitizer-v1`，虚拟触屏成功创建后、视频之前发送 |

`start` 指定 `mode:mirror/extend` 和像素 `width/height`。`resize` 更新像素 `width/height`，保持扩展显示器标识，释放按键后重启采集与编码。`ack` 回传对应配置的 `generation`，旧画面的确认不影响新编码器背压。视频上限 3840×2880；扩展模式优先放大至 1600×1200 的 HiDPI 范围，再在视频上限内保持宽高比。`pointer` 使用显示器内归一化坐标、`phase:down/move/up`、`button:0/1/2`，Pencil 另含 `kind:pencil`、`pressure`、`tiltX/tiltY`、`eraser`。键盘 `code` 为 macOS 虚拟键码，`down` 为按下状态。所有事件限于当前会话的显示器与由会话持有的按键状态。

## 原生多点触控

原生触控使用 [CoreHID.HIDVirtualDevice](https://developer.apple.com/documentation/corehid/hidvirtualdevice) 创建 HID Digitizers / Touch Screen 设备，通过 `displayUUID`、`CGDirectDisplayID` 和 `DisplayIntegrated` 绑定会话显示器，启用 `DigitizerCollectionDispatch` 多触点报告，经系统自带 HID 驱动递交触点报告。没有单独安装 DriverKit 扩展。设备注册后等待目标 `NSScreen.touchCapabilities` 包含 `.multiTouch`，最多两秒，超时不发送触控协议确认。该能力位在部分系统版本可能反映全局触屏状态，不能单独证明显示器绑定正确。设备注册成功不等于 AppKit 已接收直接触点；本机设置只有在专用临时显示器上的独立窗口收到多个 `.direct` 触点的开始、移动、结束和取消后才保存启用状态。

macOS 27 的直接触点、十指独立生命周期、取消后重用触点，以及原生 `NSButton`、`NSScrollView`、缩放和旋转识别器均由验证程序检查。扩展显示器支持横竖屏切换。启用前的检测使用当前 Host 自身签名，不以设备对象创建成功代替实际事件接收。

`start` 可带 `touchProtocol:hid-digitizer-v1`。Host 检查本机授权、创建虚拟设备并发回类型 5 确认；失败终止会话。`touch` 包含单调递增的 `sequence` 和完整 `contacts` 数组，每项为 `id`（1…10）、`phase`（`began/moved/stationary/ended/cancelled`）、显示器内归一化的 `x/y`。每帧必须包含所有已按下的手指，包括本帧抬起的手指；稳定 ID 在抬起后才能重用。重复序号、缺失触点、无效坐标和非法生命周期均终止会话。`cancelled` 使用 HID Touch + Untouch 取消标记，再清除驱动按下状态；输入释放和分辨率调整取消所有活动触点，退出销毁设备；撤销本机授权会结束当前原生触控会话。

### 权限与验证

所需权限为 `com.apple.developer.hid.virtual.device`，必须同时存在于实际代码签名和 Apple 签发的 provisioning profile 中。辅助功能授权、管理员身份和普通开发证书均不能替代它。

- 开发调试：在开发者网站的 App ID → Capabilities 中开启 **HID Virtual Device (development)**，再用 ASC 创建包含开发证书和测试 Mac 的 `MAC_APP_DEVELOPMENT` 描述文件。当前 ASC 公共 API 能读到 `HID_VIRTUAL_DEVICE_PUB`，但拒绝通过该类型启用，需使用网页开关。
- Developer ID 发行：由账户持有人通过 Capability Requests 申请 **HID Virtual Device**，批准后启用并重新生成 `MAC_APP_DIRECT` 描述文件。开发权限不会自动进入发行描述文件。操作入口见 [Apple 权限申请说明](https://developer.apple.com/help/account/capabilities/capability-requests/)。

修改 App ID 能力会使旧描述文件失效；须同步更新本机和 CI 的配置文件。不要添加已失效的 `com.apple.developer.driverkit.family.hid.virtual.device`。

独立验证 App 使用与 Host 相同的设备和报告代码，避免依赖 Sparkle 或连接实际远程会话：

```sh
Scripts/build-native-touch-probe.sh /tmp/AnywhereTouchProbe.app
/tmp/AnywhereTouchProbe.app/Contents/MacOS/AnywhereTouchProbe
```

传入 `--inspect-device` 仅注册设备 30 秒供 IORegistry 检查，不发送手指触点。

构建脚本默认使用普通临时签名，未授权时应报告无法创建设备；旁边生成的 `.entitlements` 仅为 HID 签名输入，不是授权。验证 App 标识为 `com.jintongshu.AnywhereHost.TouchProbe`；授权签名需使用匹配该 App ID、证书和测试 Mac 的描述文件，嵌入 `Contents/embedded.provisionprofile`，并保留 profile 的 application identifier 与 team identifier 权限。生产 Host 则使用自己的 App ID 和描述文件。

取得含 HID 权限的描述文件后，以下脚本检查权限、App ID、证书和有效期，再嵌入并签名；签名证书必须已连同私钥存在于本机钥匙串中：

```sh
Scripts/sign-native-touch-probe.sh /tmp/AnywhereTouchProbe.app /path/to/probe.provisionprofile CERTIFICATE_SHA1
/tmp/AnywhereTouchProbe.app/Contents/MacOS/AnywhereTouchProbe
```

验证程序仅向自己在临时显示器上的前台窗口发送触点，失焦时停止。默认检查两指生命周期和取消后复用；`--gestures` 额外检查十指、原生按钮、滚动、捏合与旋转；`--extended --gestures` 在同一扩展显示器的横屏和竖屏分辨率下分别验证。要求最终输出 `PASS AppKit received multiple direct contacts with begin/move/end/cancel`。目标应用和系统级多指手势仍需逐项验证，不能从基础识别器支持推定所有行为。

完整 Host 的开发调试签名：

```sh
ANYWHERE_DEVELOPMENT_SIGNING=1 \
ANYWHERE_SIGNING_IDENTITY=CERTIFICATE_SHA1 \
ANYWHERE_PROVISIONING_PROFILE=/path/to/host-development.provisionprofile \
bash Scripts/sign-app.sh /path/to/AnywhereHost.app
/path/to/AnywhereHost.app/Contents/MacOS/AnywhereHost --verify-native-touch --extended --gestures
```

签名脚本仅在描述文件实际授予 HID 权限时加入此 entitlement；开发签名只允许注册的测试 Mac，不能作为 Developer ID 发行包。发行仍需 Apple 批准对应权限。

独立验证 App 的 `--receive-touch-frames` 接收标准输入逐行 `touch/release` JSON（每行上限 16 KiB），将事件送到自有临时显示器上的全屏窗口。标准输出分别记录提交和真实 AppKit `observed` 回调，供隔离客户端测试使用；失焦或输入结束时释放设备。此模式不包含在产品 Host 入口中。

### 系统接口依据

macOS 27 的直接触摸由 AppKit 手势识别器接收，不能仅将 `CGEvent` 类型设为 `NSEventTypeDirectTouch` 就获得 Sidecar 行为。`NSGestureRecognizer.allowedTouchTypes`、`NSScreen.touchCapabilities`、`NSView.exclusiveGestureBehavior`、`NSScrollView.isTouchScrollingEnabled` 是接收端接口，不提供跨进程触摸注入。依据：[Apple TN3212](https://developer.apple.com/documentation/technotes/tn3212-adopting-gesture-recognizers-for-sidecar-touch-support)。

系统 `SidecarDisplayAgent` 导入的发送接口包含 `UniversalHID.HIDReportDispatcher.setInputDevices(_:for:)` 和 `dispatchReport(_:from:)`。检查框架的属性实现确认：Swift `displayIdentifier` 对应的实际 HID 属性键是 **`displayUUID`**，不是 `DisplayIdentifier`；绑定错误时即便设备注册和报告提交成功，AppKit 也收不到直接触点。产品通过 CoreHID 和系统 HID 驱动实现相同的显示器绑定，不调用未发布的 Swift 类型布局。

Apple 的 [IOHIDEventDriver 源码](https://github.com/apple-oss-distributions/IOHIDFamily/blob/main/IOHIDFamily/IOHIDEventDriver.cpp) 定义了 Digitizer Collection 的组合与 Touch + Untouch 取消语义。[IOHIDResourceUserClient 源码](https://github.com/apple-oss-distributions/IOHIDFamily/blob/main/IOHIDFamily/IOHIDResourceUserClient.cpp) 定义了虚拟设备权限检查。该路径无需 `com.apple.private.hid.client.admin`、内核扩展或关闭系统保护。

### 原生控件与显示能力

使用同一份授权签名的探针验证系统控件，无需修改 Mac 型号、Apple 厂商标识或目标应用：

```sh
/tmp/AnywhereTouchProbe.app/Contents/MacOS/AnywhereTouchProbe --adaptive-controls --extended
```

此模式输出运行时 OS 版本、HID 注册前后的 `NSScreen.touchCapabilities` 和显示器 EDR 上限，并在横竖屏分别检查原生 `NSSwitch`、`NSSlider`、`NSStepper` 的数值变化和 action。只发送直接 HID 触点，不手动扩大控件、不合成鼠标、不覆盖 AppKit 方法。输出中的 `OBSERVE` 记录按下前后控件外框与新增可见窗口；控件 action 的 `PASS` **不代表**视觉扩展通过，外框不变也不能排除控件内部绘制变化。动态放大需在目标 OS 上观察实际渲染。

Apple 的 [Touch on Mac 开发说明](https://developer.apple.com/videos/play/tech-talks/111468/) 与[设计说明](https://developer.apple.com/videos/play/tech-talks/111456/)区分原生控件、自定义控件和兼容鼠标路径。目标应用的 `NSIsTouchNative` 与事件处理方式属于接收应用，Host 无法仅靠改变设备型号让所有第三方应用采用原生控件逻辑。

触摸能力与 HDR 独立。当前协议仅传输 8-bit sRGB H.264，虚拟显示器不声明 PROLED、HDR 或 120 Hz。`CGVirtualDisplayMode` 的私有 `transferFunction`、`CGVirtualDisplayDescriptor` 的 `setDisplayInfoValue:forKey:` 是否存在，不代表其取值语义、HDR 编码与客户端呈现已验证；不得仅写入显示器身份或色域元数据就宣称具备 HDR。端到端 HDR 需要显示能力协商、采集格式、编码位深、色彩元数据和客户端 EDR 呈现共同实现。
