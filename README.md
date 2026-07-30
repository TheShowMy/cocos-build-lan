# cocos-build-lan

[English](README.en.md)

这是一个完全属于你的局域网工具项目。模板生成时会写入不可变的 UUID `tool_id` 到 `tool.json`；之后所有 Rust 源码、业务配置和控制端界面都可在本仓库修改。

> 使用 agent 或开始开发前，先阅读根目录的 [`AGENTS.md`](AGENTS.md)。它按任务路由到最少必要文档与 crate；英文版本见 [AGENTS.en.md](AGENTS.en.md)。

## 首次运行

需要 Rust 1.97.1：

```bash
# 开发模式：先构建服务端，控制端从 target/debug 同级启动
cargo build -p cocos-build-lan-server

# 本机控制端：接管已有匹配服务，或启动服务端
cargo run -p cocos-build-lan-control
```

控制端窗口标题为 `cocos-build-lan 控制端`，使用平台原生标题栏。它的“启动 / 重启 / 停止”按钮调用真实服务端：重启严格经过 readiness、prepare 和 shutdown；无法确认归属的进程不会被强杀。

若只需要运行 Web 工具，先用 dx 构建 SPA 再启动服务端：

```bash
# 构建 Web SPA（需要 dioxus-cli：cargo install dioxus-cli --locked）
scripts/build-web.sh

cargo run -p cocos-build-lan-server
# 当前 tool_id 的首选业务端口为 44588
# 浏览器打开 http://127.0.0.1:44588
```

开发模式下服务端从可执行文件同级的 `web/` 提供页面，即 `target/debug/web`；把它软链到 dx 产物目录一次即可：`ln -sfn ../dx/cocos-build-lan/release/web/public target/debug/web`。未构建 SPA 时首页只显示占位提示文本。

开发 Web UI 推荐热更新方式（改 `crates/tool-app/src` 自动重建刷新）：

```bash
# 终端 1：服务端（或用控制端 cargo run -p cocos-build-lan-control）
cargo run -p cocos-build-lan-server

# 终端 2：dx 热更新，浏览器打开 http://127.0.0.1:8080
dx serve --package cocos-build-lan-app
```

`Dioxus.toml` 已配置 `[[web.proxy]]`，dx dev server 会把 `/api/*` 代理到当前项目首选业务端口 44588。若已在控制端改过端口，同时修改此开发代理地址。

## 从哪里开始定制

| 位置 | 你拥有的内容 |
| --- | --- |
| `crates/tool-contract` | `ToolSettings`、`ToolStatus` 和任何服务/控制端共用的业务类型 |
| `crates/tool-server` | API、业务状态、配置持久化、长任务的 `RestartGuard` |
| `crates/tool-control` | 完整 Dioxus Desktop 页面、导航、业务卡片、表单和主题样式 |
| `crates/tool-app` | 浏览器业务 UI |
| `crates/tool-core` | 无 UI 的本地运行时、更新、监督和 LAN Dev。通常只在需要改变框架行为时编辑 |

最小扩展路径：在 `ToolSettings` 添加字段，在 `tool-server` 的 `GET/PUT /api/control-config` 处理它，再在 `tool-control` 的设置页渲染它。业务状态同理：扩展 `ToolStatus`、实现 `GET /api/control-status`，并在控制端添加卡片。

新增业务字段的逐步说明见 [`docs/development.md`](docs/development.md)。

`tool_id` 不属于业务配置，不能通过 `PUT /api/control-config` 修改。

## 实际 API 与数据位置

- `GET /healthz` 返回当前服务真实的 `tool_id`、协议和版本；它只监听由 `tool_id` 派生的本机回环控制端口。
- `GET /_lan/control/restart-readiness`、`POST /prepare-restart`、`POST /shutdown` 提供安全生命周期协议。
- `GET /api/control-status`、`GET/PUT /api/control-config` 是可直接改造的模板业务端点。
- Web 页面和业务 API 监听设置页中的局域网业务端口；控制接口不会挂载到该端口。

配置、数据、日志、暂存更新和 `active.json` 位于本机应用数据目录的 `<tool_id>/` 下；它们不在 release 目录，更新不会覆盖它们。服务端日志可从控制端的“日志”页读取。

## 设置、更新与发布

设置页是强类型表单，对应 `tool-contract` 中的：

```rust
ToolSettings { network: NetworkSettings { .. }, update: UpdateSettings { .. }, business: BusinessSettings { .. } }
```

在 `BusinessSettings` 增加业务字段后，同时在控制端设置表单增加对应控件即可。服务端只读写当前嵌套结构；开发期旧配置解析失败时，按错误提示删除本机 `config/tool-settings.json` 后重启生成默认值，不保留迁移分支。

更新载荷始终是**当前平台的完整 ZIP 版本包**，其中固定包含 `bin/<项目名>-server` 和 `bin/<项目名>-control`。首次安装包根目录只有一个可启动文件 `cocos-build-lan.exe`，服务端、控制端、Web 和 scripts 均位于 `bin/`。生产环境只启动根目录入口；`cargo run -p cocos-build-lan-control` 始终是开发模式，因此不会把自身交给更新器替换。

Windows x64 开发机可直接生成本地首次安装包；脚本同时保留后续 LAN Dev 所需的 Web 与运行脚本目录：

```powershell
rustup target add wasm32-unknown-unknown
rustup toolchain install nightly
powershell -ExecutionPolicy Bypass -File scripts/build-windows-release.ps1
```

1. 推送 `v*` tag 会触发 `.github/workflows/release.yml`，发布当前平台的 `bootstrap.zip`、`update.zip` 和 `manifest.json`。
2. 将 `bootstrap.zip` 解压到干净目录后，只启动根目录 `cocos-build-lan.exe`。同一 `tool_id` 会继续使用本机应用数据目录中的配置和业务数据。
3. 在控制端“设置”页填写 Release 清单 URL。控制端每六小时检查一次，也可在“更新”页立即检查；下载时校验工具 ID、包格式、平台、大小和 SHA-256。
4. 仅在 `RestartReadiness::Ready` 且设置没有未保存修改时，控制端才退出并交给启动器：启动器等待旧控制端退出，安全停止服务、原子切换 `active.json`、启动新服务并做健康检查，随后打开新版控制端。
5. 新服务健康检查失败时，启动器恢复旧指针并重新启动旧服务和旧控制端。`Deferred`、`ConfirmationRequired` 和 `Blocked` 会保留待应用包并在 UI 解释原因。

开发机用服务端和控制端一起创建 LAN Dev 完整版本包：

```powershell
$env:__COMPAT_LAYER = "RunAsInvoker"
cargo run -p cocos-build-lan-core --bin tool-dev-update -- `
  tool.json target/release/cocos-build-lan-server.exe target/release/cocos-build-lan-control.exe `
  --web dist/bootstrap/bin/web --scripts dist/bootstrap/bin/scripts `
  --listen lan --advertise 192.168.1.24 --broadcast --version 0.1.1-dev.1
```

只有在设置页打开并保存“接收可信局域网的 LAN Dev 完整版本包”后，控制端才会创建持久后台监听器。控制 TCP、默认业务 TCP 和 LAN 更新 UDP 候选都由不可变 `tool_id` 派生并自动避让，因此同机多个工具不会争用固定端口。发送器的 HTTP 端口由系统动态分配；广播仍先过滤来源、工具 ID、平台、包格式和重复版本，再下载校验。

## 排障与安全边界

- 启动失败：确认 `target/debug/cocos-build-lan-server`（开发）或 bootstrap 根目录入口与 `bin/` 内容完整；查看控制端日志页。
- 局域网无法访问：使用控制端概览展示的具体网卡 URL，并在 Windows 防火墙中允许当前设置的业务 TCP 端口。
- 更新不能应用：检查 restart readiness 的原因；`Deferred` 会等待，`ConfirmationRequired` 与 `Blocked` 需要业务明确解除。
- 更新后健康检查失败：`active.json` 自动恢复，旧服务将被重新启动；保留日志和暂存信息用于诊断。

v0.1 仅适用于可信局域网。对不可信网络，请在扩展项中加入身份认证、签名验证、传输加密和更严格的访问控制。

## 验证

```bash
cargo fmt --check
cargo test --workspace
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo metadata --format-version 1
```
