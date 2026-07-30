# 安全重启与更新

[English](restart-safety.en.md)

控制端不会强杀未知进程。它只对 `tool_id` 匹配的本机服务执行：

```text
readiness → prepare → readiness → shutdown
```

- `Ready`：允许自动重启或更新。
- `Deferred`：存在可恢复工作，保留待应用更新并等待重试时间。
- `ConfirmationRequired`：业务必须明确确认后才能继续。
- `Blocked`：业务禁止本次重启。

长任务应在 `tool-server` 里持有 `RestartGuard`，并在任务结束时释放。不要通过业务页面直接杀进程或绕过 `ToolSupervisor`。

完整更新包会先通过工具 ID、包格式、平台、大小和 SHA-256 校验并暂存。生产 launcher 执行安全停止、原子写入 `active.json`、启动新版服务和健康检查；失败时恢复旧指针并重启旧服务与控制端。配置、数据和日志必须留在 `releases/<version>/` 外。
