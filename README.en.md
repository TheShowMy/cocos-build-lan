# cocos-build-lan

Windows x64 LAN build service with a Rust TUI and Dioxus Web UI. See [the maintained guide](README.md) for configuration, development and release instructions.

```powershell
npx --yes --prefer-online --registry=http://192.168.2.33:4873 @tsixi/cocos-build-lan@latest
```

Node.js 22+, Git, uv and Cocos Creator are required. Click an address in the TUI, press S for local settings, or Q/Ctrl+C to confirm task cancellation and exit. Local paths, port and Git credentials are editable only from the service machine. Other LAN clients retain business and Feishu settings access. Passwords are write-only.

Persistent data stays in the original per-tool application data directory, outside npm's cache. The immutable tool ID does not change. There is no in-app updater; restart through npx to select the latest published version.

Build with `scripts/build-npm-release.ps1`, publish `next` using `scripts/publish-npm-release.ps1`, cold-test, then promote the same version with `-Tag latest`.
