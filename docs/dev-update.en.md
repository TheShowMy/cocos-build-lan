# LAN Dev update source

[中文](dev-update.md)

`tool-dev-update` is a local generated-project command. It serves a current-platform complete bundle containing the server and control binaries, web UI, runtime scripts, and its manifest; it never broadcasts source or a remote-control channel.

Build with `scripts/build-windows-release.ps1`, then pass `tool.json`, the server and control binaries, `--web dist/bootstrap/bin/web`, `--scripts dist/bootstrap/bin/scripts`, `--listen lan --advertise <LAN IPv4>`, and optionally `--broadcast --version <semver>`. The HTTP listener uses a system-assigned port. UDP discovery uses persistent tool-specific candidate ports derived from `tool_id`, so multiple tools and senders can coexist on one host. Receivers must explicitly save `update.lan_dev_enabled`; they filter tool ID/source/format/platform and duplicate versions, then validate size and SHA-256. Production handoff runs through the bootstrap root's `cocos-build-lan.exe`; `cargo run` development mode never replaces itself.
