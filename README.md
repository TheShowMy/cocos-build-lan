# cocos-build-lan

[English](README.en.md)

Windows x64 内网构建工具。一个 Rust 进程运行 TUI、Web 和构建任务，通过内网 npm 仓库交付完整程序。

## 运行

部署机器安装 Node.js 22+、Git、uv 和所需 Cocos Creator，然后运行：

```powershell
npx --yes --prefer-online --registry=http://192.168.2.33:4873 @tsixi/cocos-build-lan@latest
```

显式查询 `latest`；正在运行的版本不会自行替换，下次启动使用仓库最新版本。需要固定版本时使用 `@0.2.2`。内网仓库不可用时先恢复网络，不依赖缓存是否恰好存在。

TUI 显示本机和局域网地址、服务日志及任务关键进度。鼠标单击地址，或用 Tab/方向键选择后 Enter 打开；S 打开本机设置。空格暂停日志、PgUp/PgDn 翻页、End 恢复跟随。Q/Ctrl+C 先询问，确认后取消所有任务并退出。完整构建输出在 Web“构建日志”查看。`--no-tui` 使用普通日志模式，退出信号直接触发任务清理。

重复启动会提示已有实例及地址，不接管或关闭它。关闭旧实例后再启动新版。

## Web 设置

在服务机器访问本机地址或本机局域网地址，设置端口、Git 凭据、引擎路径和构建工作目录。远程访问上述区域会显示“请在服务启动的机器上访问设置。”；项目、飞书、任务参数及构建仍可远程管理，项目仍可选用已配置的引擎。

- Git Token 只写不读。留空保留，填新值替换，显式勾选清除才删除。
- 引擎可选择 `CocosCreator.exe`，也可手填安装目录或可执行文件完整路径。
- 工作目录支持系统目录窗口，必须存在且可写。推荐短路径，如 `D:\lan-workspaces`；切换不会搬移旧文件，可能需要重新初始化项目。
- 活动任务期间不能修改引擎与工作目录。端口变更保存后下次启动生效。
- 飞书配置仍开放给可信局域网用户。

## 数据与日志

`tool.json` 中的 `tool_id` 不变。默认数据位置仍是系统本地应用数据目录下 `lan-toolkit/<tool_id>/`，保留 `config/tool-settings.json`、`data/cocos-build/` 和 `logs/server.log`。配置、工作区和日志不放在 npm 安装目录中，删除 npm 缓存不会删除业务数据。

首次 npm 版本启动会把已有配置和运行态备份到数据目录的 `backups/before-npm-0.2/`，随后从配置中移除废弃更新字段。旧 releases/staging 文件不会再被使用，也不会自动清理用户磁盘文件。

`COCOS_BUILD_LAN_DATA_ROOT` 可指定独立数据根目录进行测试；不要在正常升级时变更它。

## 开发

先阅读 [AGENTS.md](AGENTS.md)。Rust 1.97.1+，Web 需要 `wasm32-unknown-unknown`、Dioxus CLI、nightly 和 pnpm。

```powershell
cargo run -p cocos-build-lan-server
# 单独开发 Web：
$env:COCOS_BUILD_LAN_DEV_ORIGIN='http://127.0.0.1:8080'
# 设置该变量的终端启动 Rust 服务；另一个终端运行：
dx serve --package cocos-build-lan-app
```

`Dioxus.toml` 的代理默认指向 44588；改端口后同步修改代理。生产包从可执行文件相邻的 `web/` 与 `scripts/` 读取资源；开发时可用 `COCOS_BUILD_LAN_WEB_ROOT` 指定 Web 产物。

- `tool-server`：Web API、TUI、任务与受控子进程。
- `tool-app`：现有 Dioxus Web 业务界面。
- `tool-contract`：本机配置类型。
- `tool-core`：工具身份、数据路径及活动任务保护。
- `obfuscation-cli`：原有混淆实现；长时间混淆通过受控工作进程执行。

## 发布

```powershell
./scripts/build-npm-release.ps1
# 使用已登录的内网 npm 账户发布测试标签：
./scripts/publish-npm-release.ps1 -Tag next
npx --yes --prefer-online --registry=http://192.168.2.33:4873 @tsixi/cocos-build-lan@next
# 冷启动验收后提升同一版本，不重新打包：
./scripts/publish-npm-release.ps1 -Tag latest
```

同时调整 Cargo 工作区和 `npm/package.json` 版本。构建脚本校验版本、资源、包体积及敏感数据排除，输出 `dist/tsixi-cocos-build-lan-<version>.tgz`。`-SkipBuild` 只用于打包已核验的同版本产物。npm 登录凭据不写入仓库。

## 验证

```powershell
cargo fmt --check
cargo test --workspace
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

另验证本机/远程权限、文件选择、TUI 退出、任务子进程清理和 npm 冷启动。仅面向可信局域网，不提供账号、配对或远程更新控制。服务日志不应记录 Git 凭据。
