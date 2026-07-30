# Tool development

[中文](development.md)

Read the generated project's [AGENTS.md](../AGENTS.md) first, then use this guide for business work.

To add a business setting, extend `BusinessSettings` with a default, keep the typed `GET/PUT /api/control-config` flow in `tool-server`, add a controlled form field in `tool-control`, and expose it to `tool-app` through a business API when needed. Never change `tool_id` or restore a JSON settings editor. During development, delete an unreadable old local settings file and recreate defaults instead of adding migration code.

To add visible business state, extend `ToolStatus`, fill it from real server state in `GET /api/control-status`, then render it in the controller and Web UI. Show a clear empty or error state rather than demo data.

Keep long tasks behind `RestartGuard`. Keep `tool-core` UI-free and change it only for lifecycle, complete-bundle, verification, rollback, or LAN Dev protocol work. `cargo run` is development mode; production uses the launcher and complete bundles. Verify formatting, tests, Clippy, checks, then real controller start/stop, settings persistence, and status display; update work also needs staging, handoff, health, rollback, saved-toggle LAN reception, and duplicate-broadcast checks.
