# GitHub 自动构建

[Build packages 工作流](../.github/workflows/build.yml) 在所有分支和 tag 的每次 `push`、每次 PR 更新以及手动 `workflow_dispatch` 时构建该次提交。不配置路径过滤，也不因新 push 取消之前的构建。

| 平台 | GitHub runner / 构建环境 | 下载产物 |
| --- | --- | --- |
| macOS arm64 | `xcode-27`（Apple Silicon，Xcode 27 SDK） | `AnywhereHost-<版本>-macos-arm64-unsigned.zip`，内含完整 `.app` |
| Linux amd64 | `ubuntu-24.04` / 原生 Debian 13 容器 | `anywhere-host_<版本>-<修订>_amd64.deb` |
| Linux arm64 | `ubuntu-24.04-arm` / 原生 Debian 13 容器 | `anywhere-host_<版本>-<修订>_arm64.deb` |

runner 架构依据 [GitHub 官方 runner 列表](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)，Linux 两种架构均在相应原生机器上编译和运行验证，不使用交叉编译或 QEMU。Linux 包针对 Debian 13 的依赖构建，不保证兼容其他发行版。

在仓库 **Actions → Build packages → 对应运行 → Artifacts** 下载。每个平台的 artifact 名含提交 SHA，保存 30 天，包旁附 `SHA256SUMS`；解压 artifact 后可运行 `sha256sum --check SHA256SUMS`（Linux）或 `shasum -a 256 --check SHA256SUMS`（macOS）。上传前须通过该平台的源码检查与包验证，失败任务不会上传包。

CI 使用只读仓库权限，不需要签名 secrets。Mac 通过显式 `Scripts/build-app.sh --unsigned` 生成未进行分发签名、未公证的 CI 包；`verify-app.py --unsigned` 只验证结构、arm64 可执行文件、版本和可选 Host 协议，不宣称签名或 Hardened Runtime 验证成功。此包供构建检查及后续签名使用，不能视为已签名安装包；正式分发仍需在 Mac 上走下面的签名流程。默认 `build-app.sh` 和 `verify-app.py` 仍要求真实签名身份与原有安全检查。

Linux CI 包可手动安装，但没有用于 OTA 的 `.deb.sig`。工作流不创建 GitHub Release、不更新 `latest` / Sparkle appcast，避免把每次提交的 CI 包推送给正式用户。正式 Release 与 OTA 继续使用维护者签名流程。

Linux Debian 13 amd64 / arm64 的 deb 构建、安装和验证见 [Linux 指南](LINUX.md)。以下内容仅适用于 macOS。

# macOS 构建、GitHub Release 与 OTA

Host 独立分发，不通过 Mac App Store 或 TestFlight。Bundle ID 为 `com.jintongshu.AnywhereHost`，目标 Apple Silicon / macOS 14+。

## 构建

```sh
swift package resolve
./Scripts/build-app.sh
python3 Scripts/verify-app.py dist/AnywhereHost.app --integration
```

默认签名构建依次选择 Developer ID Application、Apple Development 身份，也可通过 `ANYWHERE_SIGNING_IDENTITY` 指定。Sparkle 与 App 必须有相同 Team ID 才能通过 Hardened Runtime 的 library validation；不使用 ad-hoc 分发签名，也不关闭 library validation。脚本由内向外签署 Sparkle、辅助进程和 App，保留 XPC 权限，检查签名和打包结构。

当前维护者环境只有 Apple Development 证书，因此 0.1.4 是开发签名、未公证版本。Sparkle 更新包另有 Ed25519 签名，但这不能替代 Apple 公证或保证首次下载无 Gatekeeper 提示。正式 Developer ID 分发需取得相应证书，使用 `xcrun notarytool submit ... --wait` 后 `xcrun stapler staple`，再生成更新 ZIP。请勿提交证书私钥或公证凭证。

## 版本

```sh
python3 Scripts/version.py --set 0.1.5 --build 8
cargo check
```

版本同步到 Cargo 与 Xcode；build 必须递增。主界面、菜单栏和系统“关于”读取 Info.plist；后端 API 与 `--version` 读取 Cargo。构建与发布前用 `python3 Scripts/version.py` 检查一致性。

## OTA

使用 [Sparkle 官方更新机制](https://sparkle-project.org/documentation/)，固定依赖 2.10.1。Feed 位于：

`https://github.com/Xingsandesu/AnywhereHost/releases/latest/download/appcast.xml`

App 自动检查更新，也可在应用菜单点击“检查更新…”。用户选择安装后，Sparkle 验证签名、替换 App 并重新启动。App 的正常退出负责停止后端和 frpc；用户数据位于 Application Support，不包含在更新包中。隧道启动意图记录在 `tunnel-resume.json`：启动后正常退出会保留，显式“停止”会清除；下次启动自动恢复。旧版本迁入后需启动隧道一次来保存此状态。

公钥提交在 Info.plist；私钥仅保存在维护者登录 Keychain，account 为 `com.jintongshu.AnywhereHost`。已有私钥不能随意重建，否则已安装版本无法验证新包。应按 Sparkle 官方流程备份和轮换密钥。

旧版 0.1.3 没有 Sparkle，首次需要手动安装 0.1.4。此后通过应用内 OTA 更新。GitHub Release 中 ZIP、appcast 和 SHA256SUMS 作为同一个版本发布，latest 自动指向最新正式 Release。

## 发布

先完成测试、更新 `Docs/Releases/<version>.md`、构建并验证 App，再提交所有源码。推送只针对这个 Host 仓库，不涉及客户端。

```sh
./Scripts/release.sh --publish
```

脚本校验版本与签名公钥，生成 ZIP 和 appcast，用 CryptoKit 独立校验 Ed25519 签名，然后推送 `main` 和 `v<version>` tag 并创建 GitHub Release。不带参数仅生成产物，不发布；产物目录已存在时会拒绝覆盖。

发布后应从公开 URL 重新下载，验证哈希和 appcast 签名，并在旧版本上完成一次真实 OTA 升级验收。当前发布流程在维护者 Mac 上签名，不把签名私钥上传 CI。
