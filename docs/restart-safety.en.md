# Safe restart and update

[中文](restart-safety.md)

The controller never hard-kills an unknown process. It acts only on a service with the matching `tool_id` and follows `readiness → prepare → readiness → shutdown`. `Ready` permits an automatic restart; `Deferred`, `ConfirmationRequired`, and `Blocked` retain the pending update until business work permits it.

Long-running server work should hold `RestartGuard` and release it when complete. Do not bypass `ToolSupervisor` or kill processes from a business page.

Complete bundles are checked for tool ID, format, platform, size, and SHA-256 before staging. The production launcher safely stops the service, atomically switches `active.json`, starts and health-checks the new version, and restores the old service/controller on failure. Keep configuration, data, and logs outside `releases/<version>/`.
