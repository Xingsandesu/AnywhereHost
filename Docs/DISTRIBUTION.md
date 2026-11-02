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

原始 Linux CI 包没有用于 OTA 的 `.deb.sig`。主分支构建成功后，独立工作流自动发布预发布版本；正式发布模式还会签署两个 Debian 包并生成 Sparkle 更新清单。

## GitHub 签名、公证与 OTA 发布

[Sign and release packages](../.github/workflows/release.yml) 在主分支 `Build packages` 成功后自动运行。也可在 Actions 手动选择这个工作流，从 `main` 运行并填写 `build_run_id`，复用一个成功的主分支构建。默认发布预发布版本；勾选 `stable` 后发布正式 `v<版本>` 并设为 GitHub Latest。正式版要求所选构建就是当前 main 提交、版本号和 Mac 构建号都高于已发布正式版。原始构建 artifacts 必须尚未过期。

工作流先通过 GitHub API 确认构建属于本仓库、使用指定构建工作流、来自 `main` 的 push 或手动执行、已全部成功且提交仍在主分支历史中。PR、fork 和其他分支的包不会进入签名任务。签名和发布使用的脚本来自工作流启动时的主分支提交。

仓库 Actions Secrets 使用以下名称（值不放进仓库）：

| Secret | 内容 |
| --- | --- |
| `APPLE_CERTIFICATE_P12_BASE64` | 包含 Developer ID Application 证书及对应私钥的 `.p12` 的 Base64 |
| `APPLE_CERTIFICATE_PASSWORD` | `.p12` 导出密码 |
| `APPLE_TEAM_ID` | 证书所属 Apple Team ID |
| `APPLE_ID` | 公证使用的开发者账户登录邮箱 |
| `APPLE_APP_SPECIFIC_PASSWORD` | 该账户的 App 专用密码，不是账户登录密码 |
| `SPARKLE_PRIVATE_KEY` | Base64 编码的 32 字节 Ed25519 种子，同时用于 Sparkle 和 Linux OTA |

Mac runner 校验原构建包的 SHA256 后，使用临时钥匙串导入身份，确保唯一的有效 Developer ID Application 身份与 Team ID 一致。签名逻辑复用 `Scripts/sign-app.sh`，为 Sparkle 内嵌代码、框架、辅助进程和 App 启用 Hardened Runtime 与安全时间戳。Apple Developer ID 公共中间证书从 Apple 下载并校验固定 SHA256，不修改系统信任设置。

签名钥匙串在签名后删除。公证通过单独的临时钥匙串保存凭据，等待 Apple 返回 `Accepted`（最多 30 分钟），随后装订并验证公证票据、验证签名及 Gatekeeper，再重新生成 ZIP。缺少凭据、认证失败、公证拒绝或验证失败均会阻止发布。任务结束时再次清理临时钥匙串；GitHub hosted runner 随任务销毁。

发布任务不接触 Apple 凭据，只下载公证后的 Mac artifact 与原构建中的两个 Linux 包，校验哈希、架构及版本一致性，创建 `v<版本>-signed.<构建提交前12位>` 草稿。上传后的服务端 SHA256 与本地一致才公开发布，并重新匿名下载验证。已存在的同名 Release 不会被覆盖；重跑已发布构建会拒绝再次创建。发布中途失败留下的草稿需要检查后处理。

默认自动运行仍是 **prerelease**，不改变 `latest`。正式发布模式在 Mac runner 上使用固定版本 Sparkle 的 `generate_appcast` 生成新版本的 `appcast.xml`，并为两个 Debian 包生成 `.deb.sig`。私钥只写入权限受限的临时目录，结束时删除；派生公钥必须匹配 App 与 Linux 包内置公钥。Mac ZIP 使用 OpenSSL 和 CryptoKit 分别验证签名，Debian 包使用原更新器的验证逻辑复验。发布任务仅持有公钥，并再次验证所有签名、清单 URL、版本、包内置公钥和公开下载；缺失或不匹配的 OTA 签名会阻止正式发布。

本地 Linux 可以验证构建来源过滤、artifact 哈希与清单检查、工作流语法；实际 Developer ID 签名、公证和 Gatekeeper 验证以 GitHub Mac runner 的运行结果为准。

Linux Debian 13 amd64 / arm64 的 deb 构建、安装和验证见 [Linux 指南](LINUX.md)。以下内容仅适用于 macOS。

## 0.1.5 OTA 起点

0.1.5 使用 macOS 构建号 8、Debian 包版本 0.1.5-1，采用新的 Ed25519 更新身份。对应私钥保存在仓库 Actions Secret `SPARKLE_PRIVATE_KEY`，公钥同步在 `Resources/Info.plist` 和 `Linux/packaging/update-public-key.pem`。后续版本沿用这把密钥，不在每次发布时生成新密钥。

旧版本需要手动安装一次 0.1.5；从 0.1.5 起可通过应用内 OTA 升级到后续正式版。Sparkle 清单固定设置 `minimumUpdateVersion=8`，不向持有旧公钥的 Mac 版本投递新身份的更新。Linux 旧版同样需要手动跨过这个更新身份边界。新清单仅引用新发行版，不复制历史 0.1.4 清单。

正式版附带 Mac ZIP、两个 Debian 包、`appcast.xml`、两个 `.deb.sig` 和覆盖全部六个文件的 `SHA256SUMS`。签名验证和更新发现可以在隔离测试中验证；真实桌面替换安装的验收需在测试机完成，不覆盖正在运行的用户环境。

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
python3 Scripts/version.py --set 0.1.6 --build 9
cargo check
```

版本同步到 Cargo 与 Xcode；build 必须递增。主界面、菜单栏和系统“关于”读取 Info.plist；后端 API 与 `--version` 读取 Cargo。构建与发布前用 `python3 Scripts/version.py` 检查一致性。

## OTA

使用 [Sparkle 官方更新机制](https://sparkle-project.org/documentation/)，固定依赖 2.10.1。Feed 位于：

`https://github.com/Xingsandesu/AnywhereHost/releases/latest/download/appcast.xml`

App 自动检查更新，也可在应用菜单点击“检查更新…”。用户选择安装后，Sparkle 验证签名、替换 App 并重新启动。App 的正常退出负责停止后端和 frpc；用户数据位于 Application Support，不包含在更新包中。隧道启动意图记录在 `tunnel-resume.json`：启动后正常退出会保留，显式“停止”会清除；下次启动自动恢复。旧版本迁入后需启动隧道一次来保存此状态。

公钥随 App 和 Debian 包发布，当前私钥保存在 GitHub Actions Secret `SPARKLE_PRIVATE_KEY`。0.1.5 后的发布必须沿用此身份；签名前派生公钥核对，禁止自动重建或轮换。历史本地 Keychain 中 `com.jintongshu.AnywhereHost` 账户的旧密钥不适用于当前更新身份。

旧版本需手动升级到新的 0.1.5 OTA 起点。此后正式版通过同一 GitHub Latest 地址提供新版本清单、ZIP 和签名；Linux 从同一个正式 Release 发现对应架构的 `.deb` 与 `.deb.sig`。

## 发布

先递增版本和构建号、更新 `Docs/Releases/<version>.md`，提交并合并到 main，等待三个平台的 `Build packages` 全部通过。然后运行：

```sh
gh workflow run release.yml --ref main -f build_run_id=<成功构建ID> -f stable=true
```

工作流完成 Developer ID 签名、公证和 OTA 签名后，创建正式版本草稿；全部资源的服务端 SHA256 验证成功才公开并设为 Latest，再匿名下载并复验签名。普通每次 main 提交仍自动产生预发布包，正式 OTA 版本通过以上显式发布入口递增版本后发布。

`Scripts/release.sh` 保留作为维护者 Mac 的备用本地 Sparkle 流程，要求本地已导入匹配当前公钥的更新私钥；它不负责完整 Linux 多平台发布。推荐使用 GitHub 正式发布工作流。
