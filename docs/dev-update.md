# LAN Dev 更新源

[English](dev-update.en.md)

`tool-dev-update` 是生成项目内的本地命令，服务于当前平台的**完整版本包**和更新清单。版本包固定包含服务端、控制端、Web 页面与运行脚本；它不广播源代码，也不提供远程控制通道。

```powershell
rustup target add wasm32-unknown-unknown
rustup toolchain install nightly
powershell -ExecutionPolicy Bypass -File scripts/build-windows-release.ps1
$env:__COMPAT_LAYER = "RunAsInvoker"
cargo run -p cocos-build-lan-core --bin tool-dev-update -- `
  tool.json target/release/cocos-build-lan-server.exe target/release/cocos-build-lan-control.exe `
  --web dist/bootstrap/bin/web --scripts dist/bootstrap/bin/scripts `
  --listen lan --advertise 192.168.2.7 --broadcast --version 0.1.2
```

- 发布脚本先按 `crates/tool-app/editor/pnpm-lock.yaml` 构建离线 CodeMirror 6 bundle，再构建 Dioxus Web；部署机不需要 Node 或 pnpm。
- HTTP 下载端口由系统动态分配并写入 manifest；同机可同时运行多个发送器。
- `--web` 与 `--scripts` 必填，确保切换后的版本仍包含完整页面和 AST 混淆运行资源。
- `--listen lan` 必须配对 `--advertise <LAN IPv4>`，让其他机器能下载清单内的载荷 URL。
- `--broadcast` 每五秒向当前 `tool_id` 派生的 UDP 候选端口发送清单提示，且必须搭配 `--listen lan`。
- 接收端必须先在控制端设置页打开并**保存** `ToolSettings.update.lan_dev_enabled`。控制端持久绑定第一个空闲的工具专属候选端口，先过滤 `tool_id`、来源、平台和包格式；已暂存或已安装的同版本广播会被忽略。

接收端仍会校验文件大小和 SHA-256，之后按普通更新事务暂存。生产安装由根目录 `cocos-build-lan.exe` 执行服务安全停止、原子切换、健康检查并启动新版控制端；它无法确认旧控制端退出时不会切换。`cargo run` 开发模式不会替换自身。
