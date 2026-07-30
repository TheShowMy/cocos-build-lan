# LAN Dev update source

[中文](dev-update.md)

`tool-dev-update` is a local generated-project command. It serves a current-platform complete bundle containing the server and control binaries plus its manifest; it never broadcasts source or a remote-control channel.

Build both binaries, then run the command with `tool.json`, server path, control path, `--listen lan --advertise <LAN IPv4>`, and optionally `--broadcast --version <semver>`. Receivers must explicitly save `update.lan_dev_enabled`; the controller then listens in the background, filters source/tool ID/format/platform and duplicate versions, and validates size and SHA-256 before staging. Production launcher handoff performs safe stop, atomic switch, health check, and new control-app start only after the old controller exits; `cargo run` development mode never replaces itself.
