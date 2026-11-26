# cocos-build-lan：工具开发指引

[English](AGENTS.en.md)

这是单个可定制局域网构建工具，不是工具宿主或多工具平台。当前仅发布 Windows x64 npm 包。`tool.json` 的 `tool_id` 不可变；不要通过配置 API 覆盖。保持可信局域网范围，不擅自增加账号、配对或远程控制。

## 按任务阅读

- Web 设置及业务状态：[docs/development.md](docs/development.md)，修改 `tool-app`、`tool-server`，必要时修改 `tool-contract`。
- TUI 与退出：[docs/restart-safety.md](docs/restart-safety.md)，修改服务内的 `tui`、`process` 与任务生命周期。
- npm 发布：[README.md](README.md) 的发布部分，修改 `npm/` 和发布脚本。

## 约束

1. 端口、Git 凭据、引擎定义和工作目录只能在本机 Web 设置；服务端必须校验连接来源、Host、Origin。远程业务配置不能夹带这些字段。
2. Git Token 只写不读。文件/目录选择必须在服务所在机器唤起。不要使用浏览器上传路径代替真实本机路径。
3. TUI 只显示地址、运行日志与操作提示，不恢复桌面控制端、更新器、广播或版本指针。
4. npm 包只包含程序和只读资源。配置、业务数据与日志继续保存在现有工具数据目录；不要为开发期结构变化增加迁移框架。
5. 长任务必须持有活动操作保护，后台任务保留保护至任务完成。外部命令统一使用受控 `process::Command`，退出清理不能杀死无关进程。
6. 保留现有 Web 业务与构建行为，按任务做必要修改，不重做框架或引入模拟状态。

## 验证

运行 `cargo fmt --check`、`cargo test --workspace`、`cargo check --workspace`、`cargo clippy --workspace --all-targets -- -D warnings`。UI 改动需构建 Web，并验证本机和远程行为。发布需检查 npm 包、冷启动、版本和数据保留；生命周期需验证退出确认、所有任务取消、子进程收尾及异常恢复。
