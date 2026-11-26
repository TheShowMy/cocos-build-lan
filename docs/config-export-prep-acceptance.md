# 配置拉取准备项目：npm Rust 导出验收

日期：2026-10-13。

## 交付与使用

- 可导入文件：[`examples/config-export-prep.json`](../examples/config-export-prep.json)。保留原准备项目名称、全部参数及选项、固定占位符、`main.py`、`models.py` 和 `pyproject.toml`；仅替换 `lib.py`。
- 导出器源码：`D:/work/cocos/project/ddt2Core352/rust/export_config`，版本 `0.1.2`，增加 `--force-export`。
- 本轮未发布 npm、未导入或覆盖已安装准备项目，也未操作真实项目的打包产物。
- `@tsixi/export-config@0.1.2` 元包及两个平台包现已发布，`sourceCommit` 均为 `f30fece60cccbec5575a144f3a8e8abda6aac215`；Windows 从正式内网仓库空缓存下载、启动验证通过。
- 发布后再通过准备项目导入功能更新原项目。更新模式保留其原 `data/` 配置仓库缓存；本文件不包含缓存、仓库地址凭据或运行状态。

## 固定行为

- `build_mode == "release"` 使用 `release_branch`，其余使用 `test_branch`；`censor_switch == "true"` 覆盖为 `censor_branch`；最终值为空时回退到 `master`。
- `project_path` 继续由服务自动注入；脚本显式传递项目根目录和该项目的 `python/config/channel.json`。
- 配置仓库仍为准备项目目录下的 `data/<config_name>`；导出运行目录为目标项目的 `tool_data/export_config`。
- 固定使用 npm `@tsixi/export-config@0.1.2`，优先缓存；Node 优先取 `COCOS_BUILD_LAN_NODE_EXE`，通过 JS 入口调用 npm，不启动独立窗口。
- Git 使用现有认证；脚本不新增凭据传递。每次调用 `--force-export`，不传 `--force-clean`。
- 直接继承输出管道；npm/Rust 非零退出码传回准备项目。子进程不脱离原任务进程树。

## 已完成验证

- Rust：7 项原有单元测试及 4 项新增集成测试通过；格式、工作区编译、Clippy（警告视为错误）通过。
- 打包工具本身的 136 项工作区测试、格式检查、编译检查和 Clippy 全部通过，包括现有 Windows 受控进程与取消回归；本轮未修改其 Rust 源码或 Web 页面。
- 新增 Rust 测试覆盖真实隔离 Git 仓库的首次 clone、远端提交更新、默认增量跳过、覆盖及删除产物后的强制恢复、A→B→A 切换、check-only、脏仓库不被清理、导出失败不写新成功状态。
- 准备 JSON：8 项测试通过，覆盖原分支规则、Node 定位、参数数组及目录、继承日志、失败退出、缺失配置阻止运行、导出格式及凭据检查，以及继承 npx 入口和 Windows 错误日志编码。
- npm 入口和本地打包集成测试通过，包括包校验、原生二进制加载、版本读取以及父子进程树终止。
- 完整 `uv → Python 3.14 → npm → Rust → Git` 调用链通过。使用空 npm 缓存和仅服务本地测试包的临时回环 registry 首次下载；关闭 registry 后离线复用缓存，并恢复被覆盖、删除的产物。没有向内网 npm 发布测试包。
- 全链路使用中文与空格目录，未在目标项目安装 `node_modules`。
- 本机 `japan_web` 配置仓库的 `git ls-remote` 只读检查成功；未传入准备脚本凭据，未记录地址中的认证信息。
- 原 npm 测试在 Windows 模拟 macOS 路径时错误使用宿主路径分隔符，已修正为按模拟平台选择 `Path.posix`/`Path.win32`，入口测试现已通过。

## 复现

在本仓库运行：

```powershell
python -X utf8 scripts/test-config-export-prep.py
python -X utf8 scripts/test-config-export-prep-integration.py D:\work\cocos\project\ddt2Core352\rust\export_config
```

在导出器目录运行：

```powershell
cargo fmt --check
cargo test --workspace
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
node npm/test.cjs
node npm/integration-test.cjs
```

现有 npm 进程树测试已通过，但本轮未启动生产构建队列进行中途取消验收，也未运行真实项目完整配置导出及 macOS 验收。

## 真机反馈后的准备脚本修复

用户报告 `npm ERR! could not determine executable to run`。已用继承 `npm_execpath=npx-cli.js` 的隔离调用链复现相同错误：旧脚本将 npx 当作 npm，再追加 `exec`，让 npm 尝试从名为 `exec` 的包推断可执行入口。

修复仅涉及交付 JSON 的 `lib.py`：优先使用启动 Node 配套的 `npm-cli.js`；继承路径是 `npx-cli.js` 时定位其旁边的 `npm-cli.js`，不再接受任意 JS 作为 npm 入口。同时将 Python stdout/stderr 都配置为 UTF-8，修复 Windows GBK 错误日志被服务按 UTF-8 解码产生的乱码。

新增两项测试先复现入口错误及 GBK 解码失败，修复后全部通过；完整调用链测试也加入继承 npx 环境，在冷缓存和离线热缓存下通过。更新原准备项目即可，导出器及打包工具的 npm 包无需重新发布。
