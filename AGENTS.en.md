# cocos-build-lan: tool development guide

[中文](AGENTS.md)

Start here, then read only the route for the task at hand. This generated repository owns one customizable LAN tool, not a multi-tool host. During development, configuration, manifests, LAN Dev payloads, `active.json`, installation layout, and launch behavior are intentionally breaking; do not add legacy migration paths before the stable release.

`tool_id` in `tool.json` is immutable. It must match settings-independent runtime identity, manifests, broadcasts, and control operations. v0.1 assumes a trusted LAN and uses `tool_id` isolation plus SHA-256 transfer checks only.

| Task | Read first | Change first |
| --- | --- | --- |
| Business settings or status | [docs/development.md](docs/development.md) | `tool-contract`, `tool-server`, `tool-control`, `tool-app` |
| Control UI | [docs/development.md](docs/development.md) | `tool-control` |
| Server API or restart protection | [restart safety](docs/restart-safety.en.md) | `tool-server`, then `tool-contract` if shared types are needed |
| Updates, LAN Dev, release, rollback | [README update section](README.en.md), [LAN Dev](docs/dev-update.en.md) | `tool-core` only when protocol behavior must change |
| Template maintenance | the source template repository's `docs/development.en.md` | `template/`, then regenerate and validate |

Put business configuration in `BusinessSettings` and visible business state in `ToolStatus`. Update the server API, controlled desktop form/status cards, and Web UI together. Do not restore a JSON settings editor or replace runtime data with demo values.

`cargo run -p cocos-build-lan-control` is development mode. Production starts from the bootstrap root's `cocos-build-lan.exe`; complete control-app updates are handed to that launcher. For ordinary changes run `cargo fmt --check`, `cargo test --workspace`, and `cargo check --workspace`, then exercise the real control app. Update or lifecycle work also requires health, graceful restart, bundle handoff, rollback, and `active.json` checks.
