# 工程约束

- 提交作者与提交者使用 GitHub 隐私邮箱，维护者统一为 `Xingsandesu <55939146+Xingsandesu@users.noreply.github.com>`；禁止写入维护者个人或工作邮箱。启用 `git config core.hooksPath .githooks`，提交与推送前由 `Scripts/verify-git-privacy.py` 检查。
- 产品：通过原生 Host 协议为 AnyWhereCoding 提供自托管后端，支持多设备远程 Codex 工作流。
- 本机界面仅管理服务状态、连接、权限和生命周期；会话、项目、模型与审批交互由客户端承担。
- 保留实际 Codex Home、项目和会话 ID、cwd 与配置；运行时接入顺序见 [架构](Docs/ARCHITECTURE.md)。独立启动的 app-server 不代表已控制桌面进程。
- 两端统一使用 iroh 原生连接与流、自建 relay；Host 版本为 1.1.6（56），不保留兼容实现。
- 远程操作仅使用协议方法和显式资源接口；禁止斜杠命令解析、提示词控制动作及客户端传入的 shell 命令字符串。
- 源码保持模块化；macOS 交付单个 `.app`，内含 Rust 后端和 iroh，不分发 Codex。
- 界面能力必须对应运行时实际支持；不支持的操作明确报错，禁止伪造成功状态和仓库结果。
- 相关变更执行 [贡献指南](CONTRIBUTING.md) 中的检查；测试不得修改已有用户会话，真实 Codex 只读检查与显式启用的推理检查分开。
- 文档仅保留已确定的约束、配置、接口与操作命令；同一规则只在一处定义，其他位置引用。
- 除 README、CHANGELOG 可使用中英文外，文档正文统一中文；保留协议标识、代码和命令原文。
- 禁止新增日期审查报告、性能报告、验证流水账、过程说明和重复发布记录；版本变更统一写入 CHANGELOG，测量输出写入临时目录。
