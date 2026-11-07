# 构建与分发

## macOS

运行需要 macOS 27+；源码构建另需 Xcode 27+ 和 Rust。交付单个 `.app`，内含 SwiftUI 界面、Rust 后端、iroh 和权限检查程序；Codex 由用户另行安装，应用通过 Sparkle 更新。

```sh
Scripts/build-app.sh --unsigned
```

正式签名使用 `Scripts/build-app.sh`，签名与公证分别由 `Scripts/sign-app.sh`、`Scripts/notarize-app.sh` 执行。未签名构建用于开发检查，生产网络接入要求见[应用验证](RELAY.md#应用验证)。

| 环境变量 | 用途 |
| --- | --- |
| `CARGO` | Cargo 可执行程序 |
| `CARGO_TARGET_DIR` | Rust 输出目录 |
| `ANYWHERE_DERIVED_DATA` | Xcode 缓存目录 |
| `ANYWHERE_OUTPUT_DIR` | 打包输出目录 |
| `ANYWHERE_SIGNING_IDENTITY` | 签名身份 |
| `ANYWHERE_PROVISIONING_PROFILE` | 本机 Developer ID provisioning profile |
| `APPLE_PROVISIONING_PROFILE_BASE64` | CI 配置文件 Secret |

构建脚本从 Cargo 和 Xcode 读取产物位置。外层 App 嵌入 provisioning profile，并启用 `com.apple.developer.devicecheck.app-attest-opt-in = [CDhash]`；原生 macOS 不使用 iOS 的 `com.apple.developer.devicecheck.appattest-environment`。

## 发布

签名、公证与 Gatekeeper 检查通过后，流水线登记代码哈希，成功后才发布。已发布包可用 `Register published Host` 工作流重新登记；接入目录与 OIDC 限制见[网络接入](RELAY.md#发布登记)。

自动 push 构建与预发布交付 macOS 公证安装包。正式版将当前 `main` 成功的 `Build packages` 运行 ID 作为 `build_run_id` 传给 `Sign and release packages`，设置 `stable=true`。发布版本与 Mac 构建号必须高于现有正式版，产物包括公证安装包、Sparkle 更新清单与校验和：

```sh
gh workflow run release.yml --ref main -f build_run_id="$BUILD_RUN_ID" -f stable=true
```

`Scripts/release.sh` 用于本机准备和发布 Mac 更新包。

本机没有 Developer ID 私钥时，可通过 Xcode 已登录且获授权的团队执行自动发行签名。使用 `asc xcode archive` 归档，传入 `CODE_SIGN_STYLE=Automatic`、`CODE_SIGN_IDENTITY=Apple Development`、`DEVELOPMENT_TEAM=775Y7KV7KU` 与 `-allowProvisioningUpdates`；导出 `.app`：

```sh
xcodebuild -exportArchive -archivePath /tmp/AnywhereHost.xcarchive \
  -exportPath /tmp/AnywhereHost-export \
  -exportOptionsPlist Scripts/DeveloperIDExportOptions.plist -allowProvisioningUpdates
```

将导出 App 压缩后执行 `asc notarization submit --file 包路径 --wait`，成功后使用 `xcrun stapler staple`、`xcrun stapler validate` 与 `spctl --assess --type execute` 验证 App。按[本机发行登记](RELAY.md#发布登记)配置最终代码哈希并实测生产接入，再以 `ANYWHERE_RELEASE_APP` 指向此 App 执行 `Scripts/release.sh --publish`。

Sparkle 私钥保存在钥匙串账户 `com.jintongshu.AnywhereHost`，公钥须与 App 匹配。轮换时设置 `ANYWHERE_PREVIOUS_APP` 指向已安装的旧正式版；更新验证会检查旧版指定的 Apple 签名身份、新版 Ed25519 签名和递增构建号。GitHub 的 `SPARKLE_PRIVATE_KEY` 也须同步，私钥不进入仓库或日志。

iOS 客户端要求 iOS 27.1+，通过 Git 子模块固定共享传输源码。版本检查与测试见[贡献指南](../CONTRIBUTING.md)。
