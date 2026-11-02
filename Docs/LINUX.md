# Debian 13 Host

构建目标为 **Debian 13（trixie）amd64 / x86_64 和 arm64 / aarch64，GNOME Wayland 桌面**，并提供 X11 共享后端。GTK 4 / PyGObject 管理界面复用项目的 Rust Host 与协议 v2；Codex 由用户自行安装，不随包分发。现有本地实测记录来自 amd64；arm64 的原生构建与隔离测试由新增 GitHub CI 执行，结果以对应运行记录为准。

## 与 macOS Host 对应的功能

| macOS Host | Debian Host |
| --- | --- |
| 服务状态、版本、运行时、启停与重新连接 | GTK 服务概览，复用 Rust API |
| 首次欢迎 → 权限 → 就绪向导 | 三步向导；必需目录可读写且 Codex 已连接才记录完成；跳过不记录完成 |
| 启动后复查权限 | 已完成向导的安装自动复查 Host 数据、实际 Codex Home 与选定目录 |
| 菜单栏、关窗驻留、显式退出 | StatusNotifierItem / DBusMenu、应用菜单、关窗驻留、重新打开；退出只停止自有子进程 |
| 设备二维码、复制链接与撤销 | v2 一次性配对、提前刷新、使用后刷新、地址变化清除、设备撤销 |
| frps 配置、CA、令牌重用、日志、恢复 | 使用相同后端与配置；不会把进程存活显示成公网可达 |
| 屏幕共享与独立登录 | TigerVNC 1.16.2 的 Wayland portal / X11 共享，独立 VNC 密码，通过相同 RFB-over-WSS 协议 |
| 屏幕与输入授权 | 本机“授权并检查”触发桌面门户授权；授权失败、取消、缺失运行时均显示实际错误 |
| 文件权限检查、选择目录、取消、系统设置 | GTK 目录选择器、可取消且限时的独立 Rust probe、GNOME 隐私与网络设置 |
| 打开日志、主机数据、显示应用 | GTK 日志视图和文件管理器入口 |
| 自动检查、手动检查、签名下载、安装重启 | GitHub Release 的 deb + Ed25519 detached signature；Polkit 授权安装与重启 |

Linux 没有 macOS 的 TCC“完全磁盘访问”授权位。文件和自动化访问使用当前用户权限与 Codex 原生审批；屏幕和输入使用桌面门户的真实授权，不合成 macOS 授权结果。Host 不提供项目浏览器、会话编辑器、模型选择或审批 UI，这些交互仍由客户端实现。

## 安装与使用

```sh
sudo apt install ./anywhere-host_0.1.4-3_amd64.deb
anywhere-host-gtk
```

也可以从应用菜单启动。完成向导前，需要已经安装、登录 Codex App 或 CLI。已有配置会在打开应用时自动启动服务；新安装在向导中启动。关窗后服务继续运行；应用菜单或状态图标中的“退出”会关闭其自有 Host、frpc、桌面共享及独立 Codex 子进程，不停止外部共享 Codex。

arm64 使用同版本的 `_arm64.deb`；可用 `dpkg --print-architecture` 确认系统架构。每次 push 的开发构建可从 [GitHub Actions artifacts](DISTRIBUTION.md#github-自动构建) 下载。

GNOME 的状态图标需要 `gnome-shell-extension-appindicator`（包推荐依赖）。安装后如当前会话尚未发现扩展，重新登录，再点击“启用 GNOME 状态图标”。其他支持 StatusNotifierItem 的桌面直接显示图标。没有托盘宿主时仍可后台驻留，并通过应用菜单重新打开窗口；UI 明确显示托盘连接状态。

多台 Host 可以共用一个 frps：代理名称根据各自的 Host 证书指纹自动生成，重启后保持稳定；每台 Host 必须使用不同的公网映射端口。旧的固定 `anywhere-host` 名称在下次启动隧道时自动迁移，现有地址、令牌与端口保留。

### 桌面共享

在“远程桌面”填写独立的 **恰好 8 位可打印 ASCII VNC 密码**（半角字母、数字或符号），点击“保存密码”，确认显示“密码已保存”后，再点击“允许已配对设备连接”。保存密码不依赖服务是否启动或共享是否开启，也不会自动开启共享；共享开启后仍可修改密码，新连接使用新密码，已有连接不因此断开。保存失败会在密码栏下显示原因并保留输入，便于修改。

在密码栏再次填写已保存的密码后点击“授权并检查屏幕与输入”，并在本机系统弹窗中选择屏幕、允许控制。此按钮检查已有密码与授权，不负责保存密码。也可以由首次已认证的客户端连接触发授权。桌面门户决定能否记住授权；Host 不绕过系统弹窗。

Rust 根据运行会话选择打包的 `w0vncserver`（Wayland）或 `x0vncserver`（X11）。helper 只监听状态目录下的 0600 Unix socket，不开放 TCP VNC 端口。远程设备经 TLS 1.3、Host v2 设备证明和独立 VNC 认证访问。最多两个远程桌面连接；关闭开关、退出 Host 或撤销设备会终止相应连接。保留现有 WSS 心跳、超时和流量日志，不转发剪贴板、音频或文件。

`/v1/desktop` 报告实际 helper、错误、enabled、reachable 和活动连接数。`reachable` 表示本地 RFB listener 可连接，不等于桌面门户已经授权；UI 的授权检查在完成真实 RFB 登录并取得桌面尺寸后才报告成功。没有桌面会话或 helper 时 `supported` 为 false，连接明确失败。

### 数据与运行时

状态默认 `$XDG_STATE_HOME/AnywhereHost`；变量缺失、为空或非绝对路径时使用 `~/.local/state/AnywhereHost`。`--state-dir` 可显式指定。状态目录 0700，密钥、证书、配置和桌面密码文件 0600。卸载保留用户数据。

保留 `$CODEX_HOME`，默认 `~/.codex`。优先使用该 Home 下的已有 `app-server-control/app-server-control.sock`（可显式配置）；仍需要安装的可执行文件生成协议 schema。自动查找常见 App 安装目录 `/opt/Codex/resources/codex`、`/opt/codex/resources/codex`、`~/.local/share/codex/resources/codex`，再查找 PATH、`~/.local/bin`、`~/.cargo/bin`、`/usr/local/bin` 与 `/usr/bin` 的 CLI。非标准安装在“运行时与权限”指定实际路径。

独立 app-server 只共享磁盘登录、配置和历史，不代表接管桌面进程。原有项目/会话 ID、cwd、配置和跨进程写入限制保持不变。管理请求只连接回环地址，使用逐连接证书固定验证及 v2 HMAC 证明；默认 `127.0.0.1:9443`，远程连接通过 frp。配置保存也取得后端实例锁，不覆盖外部运行的 Host 配置。

## 更新与发布

包包含 GTK、Rust、checksum-pinned frpc 0.71.1，以及从 checksum-pinned TigerVNC 1.16.2 源码构建的三个独立 helper。附带 frp/TigerVNC 许可证和 TigerVNC 完整对应源码与构建脚本。Rust/C++ 构建使用路径映射，不将构建者 Home 写入 ELF。

自动检查仅查询 Release，不自动下载或安装。发布方上传 `anywhere-host_<版本>-<修订>_<amd64或arm64>.deb` 与同名 `.deb.sig`；更新器按本机 Debian 架构选择包，下载验证和特权安装再次拒绝架构不匹配的包。签名使用与 macOS Host 相同的 Ed25519 公钥（来自 Info.plist）；没有合法签名的发行包不会显示成可安装更新。

```sh
python3 Scripts/sign-deb.py dist/anywhere-host_0.1.5-1_amd64.deb --key /secure/path/host-ed25519.pem
```

私钥不进入仓库、构建镜像或 deb。安装时用户通过 Polkit 授权；root helper 首先把输入复制到私有临时目录，再验证签名、包名、架构与版本，拒绝降级、符号链接或超大输入，然后由 APT 安装。用户状态保留。取消或失败会恢复原自有服务；成功后重新打开应用。也可手动安装新 deb。

## 构建与验证

原生 Debian 13 amd64 或 arm64，Rust stable。依赖：

```sh
sudo apt install build-essential dpkg-dev curl git openssl python3 python3-gi \
  python3-qrcode python3-cryptography gir1.2-gtk-4.0 xvfb xauth dbus-daemon \
  cmake pkg-config libx11-dev libxext-dev libxdamage-dev libxfixes-dev \
  libxrandr-dev libxtst-dev zlib1g-dev libpixman-1-dev libjpeg62-turbo-dev \
  libpam0g-dev libgnutls28-dev libglib2.0-dev libpipewire-0.3-dev uuid-dev \
  libwayland-dev libxkbcommon-dev
cargo build --locked --bins
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
python3 Scripts/verify-host.py
python3 Scripts/fetch-frpc.py
python3 Scripts/verify-frp.py
python3 Scripts/build-deb.py
xvfb-run -a dbus-run-session -- python3 Scripts/verify-deb.py dist/*.deb --ui --desktop
```

`verify-deb.py` 会提取完整包并执行 Python 管理、更新、密码测试及所选 GTK / 桌面检查。密码测试需要包内真实的 `vncpasswd`，因此全新工作区须先打包；只有 `cargo build` 的目录不包含该工具。

也可用 `build-deb.py --desktop-container` 在 Debian 容器中构建桌面 helper，无需给宿主安装 C++ 开发依赖。构建不需 root；包架构、ELF 依赖和 Cargo 版本自动读取，限定原生 Debian 13 amd64 / arm64。包验证检查六个 ELF 可执行文件的实际架构及包元数据，并运行提取后的服务与桌面 fixture。

`verify-linux-desktop.py` 仅允许在显式标记的隔离 Xvfb 中运行，验证真实画面、键盘、鼠标、独立 VNC 认证、会话上限和关闭共享。`smoke-real-codex.py` 是独立只读检查，不启用 `--inference`，共享模式不执行 `thread/resume`。SwiftUI / Sparkle 构建仅适用于 macOS；当前 Linux 主机没有 Swift，不能据此声称 macOS 构建已通过。

本次已执行的验证及手工验收边界见 [验证记录](VERIFICATION.md#debian-13-amd64-2026-11-02)。
