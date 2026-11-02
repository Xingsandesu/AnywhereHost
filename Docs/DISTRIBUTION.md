Linux Debian 13 amd64 的 deb 构建、安装和验证见 [Linux 指南](LINUX.md)。以下内容仅适用于 macOS。

# macOS 构建、GitHub Release 与 OTA

Host 独立分发，不通过 Mac App Store 或 TestFlight。Bundle ID 为 `com.jintongshu.AnywhereHost`，目标 Apple Silicon / macOS 14+。

## 构建

```sh
swift package resolve
./Scripts/build-app.sh
python3 Scripts/verify-app.py dist/AnywhereHost.app --integration
```

构建脚本依次选择 Developer ID Application、Apple Development 身份，也可通过 `ANYWHERE_SIGNING_IDENTITY` 指定。Sparkle 与 App 必须有相同 Team ID 才能通过 Hardened Runtime 的 library validation；不再使用 ad-hoc 打包，也不关闭 library validation。脚本由内向外签署 Sparkle、辅助进程和 App，保留 XPC 权限，检查签名和打包结构。

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
