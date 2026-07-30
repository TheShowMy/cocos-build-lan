# cocos-build-lan：工具开发指引

[English](AGENTS.en.md)

本文件是后续 agent 和开发者进入此生成项目时的第一入口。按当前任务读取最少必要内容，不要先通读所有实现或重做框架。

## 先确认边界

- 这是一个**单个可定制局域网工具**，不是工具宿主机、注册中心、反向代理或多工具平台。
- 当前仍处开发期：配置结构、更新 manifest、LAN Dev 载荷、`active.json`、安装布局和启动方式均允许破坏性调整；不要为旧状态增加迁移或兼容分支。稳定版发布后删除这条规则并单独设计兼容策略。
- `tool.json` 内的 `tool_id` 不可变。业务设置、API、更新清单、广播和运行实例都必须与它一致；不要通过配置 API 覆盖它。
- v0.1 只假设可信局域网：用 `tool_id` 隔离工具，用 SHA-256 发现传输损坏；不要擅自加入账号、配对、签名或远程控制。

## 按任务阅读

| 你的任务 | 先读 | 主要修改位置 |
| --- | --- | --- |
| 新增业务配置或业务状态 | [`docs/development.md`](docs/development.md) | `crates/tool-contract`、`tool-server`、`tool-control`、`tool-app` |
| 新增或修改 Web 业务页面 | [`README.md`](README.md) 的“从哪里开始定制” | `crates/tool-app` |
| 修改本机控制端页面、表单或状态卡片 | [`docs/development.md`](docs/development.md) | `crates/tool-control` |
| 增加服务 API、任务或重启保护 | [`docs/restart-safety.md`](docs/restart-safety.md) | `crates/tool-server`，必要时 `tool-contract` |
| 调整更新、LAN Dev、发布或回滚 | [`README.md`](README.md) 的“设置、更新与发布”、[`docs/dev-update.md`](docs/dev-update.md) | 仅在确有框架协议需求时修改 `tool-core` |
| 修改此模板本身 | 生成项目外的模板仓库 `docs/development.md` | `template/`；必须重新生成项目验证 |

## 开发路径

1. 业务字段放在 `BusinessSettings`，控制端可见业务摘要放在 `ToolStatus`；更新偏好保留在 `UpdateSettings`。
2. 同步修改服务端的配置读写或业务 API、控制端的受控表单或真实状态卡片，以及需要时的 Web UI。
3. 不要把设置退回 JSON 编辑器，也不要用模拟数据替代服务、PID、版本、更新或日志等真实运行状态。
4. 优先改用户项目拥有的四个 crate：`tool-contract`、`tool-server`、`tool-control`、`tool-app`。`tool-core` 只放无 UI 的通用运行时和协议能力。
5. `cargo run -p cocos-build-lan-control` 是开发模式。生产环境只能从首次安装包目录的 `cocos-build-lan-launcher` 启动；完整控制端更新由 launcher 接管，不能在开发模式自行替换控制端。

## 验证

普通业务改动至少运行：

```bash
cargo fmt --check
cargo test --workspace
cargo check --workspace
```

实际启动控制端，确认服务启动、配置保存、真实状态读取和主题切换可用。涉及生命周期或更新时，额外验证健康检查、`readiness → prepare → readiness → shutdown`、完整包交接、健康失败回滚和 `active.json` 一致性。
