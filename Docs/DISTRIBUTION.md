# GitHub 自动构建

[Build packages 工作流](../.github/workflows/build.yml) 在所有分支和 tag 的每次 `push`、每次 PR 更新以及手动 `workflow_dispatch` 时构建该次提交。不配置路径过滤，也不因新 push 取消之前的构建。

| 平台 | GitHub runner / 构建环境 | 下载产物 |
| --- | --- | --- |
| macOS arm64 | `xcode-27`（Apple Silicon，Xcode 27 SDK） | `AnywhereHost-<版本>-macos-arm64-unsigned.zip`，内含完整 `.app` |
| Linux amd64 | `ubuntu-24.04` / 原生 Debian 13 容器 | `anywhere-host_<版本>-<修订>_amd64.deb` |
| Linux arm64 | `ubuntu-24.04-arm` / 原生 Debian 13 容器 | `anywhere-host_<版本>-<修订>_arm64.deb` |

runner 架构依据 [GitHub 官方 runner 列表](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)，Linux 两种架构均在相应原生机器上编译和运行验证，不使用交叉编译或 QEMU。Linux 包针对 Debian 13 的依赖构建，不保证兼容其他发行版。

在仓库 **Actions → Build packages → 对应运行 → Artifacts** 下载。每个平台的 artifact 名含提交 SHA，保存 30 天，包旁附 `SHA256SUMS`；解压 artifact 后可运行 `sha256sum --check SHA256SUMS`（Linux）或 `shasum -a 256 --check SHA256SUMS`（macOS）。上传前须通过该平台的源码检查与包验证，失败任务不会上传包。

构建工作流使用只读仓库权限，不需要签名 secrets。Mac 通过显式 `Scripts/build-app.sh --unsigned` 生成未进行分发签名、未公证的 CI 包；`verify-app.py --unsigned` 只验证结构、arm64 可执行文件、版本和可选 Host 协议，不宣称签名或 Hardened Runtime 验证成功。此包供构建检查及后续签名使用，不能视为已签名安装包。默认 `build-app.sh` 和 `verify-app.py` 仍要求真实签名身份与原有安全检查。

Linux CI 包可手动安装，但没有用于 OTA 的 `.deb.sig`。构建工作流本身不创建 GitHub Release；主分支构建成功后，由独立的签名工作流发布预发布版本。正式 Release 与 OTA 继续使用维护者签名流程。

## GitHub 签名、公证与预发布

[Sign and release packages](../.github/workflows/release.yml) 在主分支 `Build packages` 成功后自动运行。也可在 Actions 手动选择这个工作流，从 `main` 运行并填写 `build_run_id`，复用一个成功的主分支构建。原始构建 artifacts 必须尚未过期。

工作流先通过 GitHub API 确认构建属于本仓库、使用指定构建工作流、来自 `main` 的 push 或手动执行、已全部成功且提交仍在主分支历史中。PR、fork 和其他分支的包不会进入签名任务。签名和发布使用的脚本来自工作流启动时的主分支提交。

仓库 Actions Secrets 使用以下名称（值不放进仓库）：

| Secret | 内容 |
| --- | --- |
| `APPLE_CERTIFICATE_P12_BASE64` | 包含 Developer ID Application 证书及对应私钥的 `.p12` 的 Base64 |
| `APPLE_CERTIFICATE_PASSWORD` | `.p12` 导出密码 |
| `APPLE_TEAM_ID` | 证书所属 Apple Team ID |
| `APPLE_ID` | 公证使用的开发者账户登录邮箱 |
| `APPLE_APP_SPECIFIC_PASSWORD` | 该账户的 App 专用密码，不是账户登录密码 |

Mac runner 校验原构建包的 SHA256 后，使用临时钥匙串导入身份，确保唯一的有效 Developer ID Application 身份与 Team ID 一致。签名逻辑复用 `Scripts/sign-app.sh`，为 Sparkle 内嵌代码、框架、辅助进程和 App 启用 Hardened Runtime 与安全时间戳。Apple Developer ID 公共中间证书从 Apple 下载并校验固定 SHA256，不修改系统信任设置。

签名钥匙串在签名后删除。公证通过单独的临时钥匙串保存凭据，等待 Apple 返回 `Accepted`（最多 30 分钟），随后装订并验证公证票据、验证签名及 Gatekeeper，再重新生成 ZIP。缺少凭据、认证失败、公证拒绝或验证失败均会阻止发布。任务结束时再次清理临时钥匙串；GitHub hosted runner 随任务销毁。

发布任务不接触 Apple 凭据，只下载公证后的 Mac artifact 与原构建中的两个 Linux 包，校验哈希、架构及版本一致性，创建 `v<版本>-signed.<构建提交前12位>` 草稿。上传后的服务端 SHA256 与本地一致才公开发布，并重新匿名下载验证。已存在的同名 Release 不会被覆盖；重跑已发布构建会拒绝再次创建。发布中途失败留下的草稿需要检查后处理。

这些版本是供手动安装的 **prerelease**，不改变 `latest`，不生成 Sparkle appcast 或 Linux OTA 签名。Apple 公证不会替代 Sparkle 的 Ed25519 更新签名。正式 OTA 的现有私钥继续留在维护者 Mac，不自动生成或轮换。

本地 Linux 可以验证构建来源过滤、artifact 哈希与清单检查、工作流语法；实际 Developer ID 签名、公证和 Gatekeeper 验证以 GitHub Mac runner 的运行结果为准。

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

最初的正式 0.1.4 是 Apple Development 开发签名、未公证版本。新的 GitHub 签名预发布使用上面的 Developer ID 与公证流程；历史 Release 的包不会被替换。Sparkle 更新包另有 Ed25519 签名，但这不能替代 Apple 公证或保证首次下载无 Gatekeeper 提示。请勿提交证书私钥或公证凭证。

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

发布后应从公开 URL 重新下载，验证哈希和 appcast 签名，并在旧版本上完成一次真实 OTA 升级验收。此处的正式 OTA 发布流程在维护者 Mac 上执行；GitHub 自动预发布仅使用 Apple 签名与公证凭据，不持有 Sparkle OTA 私钥。
