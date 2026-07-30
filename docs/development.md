# 工具开发

[English](development.en.md)

先阅读项目根目录的 [`AGENTS.md`](../AGENTS.md)，再按这里的路径开发业务功能。

## 新增一个业务配置字段

以新增“业务问候语”外的字段为例：

1. 在 `crates/tool-contract` 的 `BusinessSettings` 增加字段和默认值。
2. 让 `tool-server` 的 `GET/PUT /api/control-config` 继续读写该强类型设置；业务逻辑从 `settings.business` 读取它。
3. 在 `tool-control` 设置页增加受控字段和说明，并在保存、还原后确认值来自服务端。
4. 若浏览器业务界面需要该字段，通过业务 API 读取并在 `tool-app` 使用它。

不要修改 `tool_id`，不要把整个设置对象暴露成 JSON 文本编辑器。当前开发期的旧本地设置无法解析时，应删除本机配置文件并重启生成默认值。

## 新增一个业务状态

1. 在 `ToolStatus` 增加可展示的字段。
2. 在 `tool-server` 的 `GET /api/control-status` 从真实业务状态填充它。
3. 在 `tool-control` 添加状态卡片或表格；无数据时显示明确空状态或错误原因。
4. 按需要让 `tool-app` 使用同一业务 API，不复制演示数据。

## 生命周期与更新边界

- 长任务持有 `RestartGuard`，避免控制端在不可安全重启时中断业务；详见 [`restart-safety.md`](restart-safety.md)。
- `tool-core` 是无界面框架能力。仅当服务生命周期、完整版本包、校验、回滚或 LAN Dev 协议必须变化时才编辑它。
- 开发时用 `cargo run`；生产时由 launcher 启动控制端与服务端。完整更新包固定包含二者，更新流程不应被业务代码绕过。

## 完成定义

运行格式、测试、Clippy 和构建检查，再实际验证控制端能启动/停止服务、保存新增设置并读取新增状态。涉及更新时还要验证暂存、交接、健康检查和失败回滚。LAN Dev 变更还要验证保存开关后才接收、关闭后停止接收，以及重复广播不会重复下载。
