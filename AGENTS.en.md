# Development entrypoint

See [AGENTS.md](AGENTS.md) for the maintained project rules. This repository is one trusted-LAN tool, distributed through npm for Windows x64. Do not alter its tool ID or move persistent data into npm's cache.

Use `tool-server` for the integrated TUI, web API and managed subprocesses; `tool-app` for Web UI; `tool-contract` for local settings; `tool-core` for identity, paths and active-task protection. Machine settings require server-side peer/Host/Origin checks. Git passwords are write-only. Keep business settings available over LAN.

Run fmt, workspace tests/checks, Clippy, Web build and focused UI/lifecycle/package acceptance. Do not reintroduce the desktop controller, launcher, update manifests, LAN broadcasts or active-version pointer.
