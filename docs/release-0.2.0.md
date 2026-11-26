# 0.2.0 发布验收记录

日期：2026-10-13。目标：Windows x64。所有运行验收使用独立临时数据目录，未改动正常使用的数据目录或独立打开的 Cocos Creator。

## 发布结果

- 内网包：`@tsixi/cocos-build-lan@0.2.0`，`next` 和 `latest` 均为 `0.2.0`。
- 安装包：`dist/tsixi-cocos-build-lan-0.2.0.tgz`，5,085,384 字节。
- SHA-512：`syoxSFyB/Ok3MJgxOp0mxmEFM3hjp/QK7PtNMi2iQZh2hlT9+fex/9vWtJ/m84QU+NBYLYNm7ApLU/50Z8NyKQ==`。
- 先发布 `0.2.0-rc.1` 并冷启动，再发布 `0.2.0` 的测试标签；验收后比较 tarball 完整性并提升 `latest`。

```powershell
npx --yes --prefer-online --registry=http://192.168.2.33:4873 @tsixi/cocos-build-lan@latest
```

## 已通过

- `cargo fmt --check`、`git diff --check`、全工作区 115 个测试、`cargo check --workspace`、Clippy `-D warnings`。
- Web 生产构建、Rust release 构建、npm 包资源/体积检查。生产 Web 显式关闭调试符号，规避当前 Windows wasm-opt 的 DWARF 崩溃；发布构建清除旧哈希资源。
- 从干净目录和全新 npm 缓存启动候选版；使用相同缓存执行 `@latest`，实际运行进程来自 npm 缓存，版本由候选版变为 `0.2.0`。正在运行的候选版未被新包替换。
- 重启后端口由 45681 变为 45682；配置、引擎、项目、Git 初始化的工作区和准备脚本保留；Web 页面版本与 Rust 健康检查一致。
- 本机回环地址及本机网卡地址的设置权限；异源写入拒绝、无 Origin 写入拒绝、公共保存接口注入引擎字段拒绝；Git Token 不回传、留空保留及显式清除。
- 原生工作目录选择、中文和空格路径、取消保持原值、重复选择请求返回 409；原生引擎文件选择和已有 Creator 可执行文件校验。
- 真实 Git 初始化；运行真实 uv/Python 准备脚本期间，引擎和工作目录修改返回 409。
- TUI 伪终端测试：Q 默认取消、Ctrl+C 确认、日志暂停、确认退出、日志落盘；`--no-tui` 的 Ctrl+C 退出。重复启动提示已有实例。
- 正常退出及强制结束服务时，准备脚本和派生 Python 子进程均被回收；用户独立打开的 Creator 保持运行。
- npm 安装目录中的 AST 脚本能加载固定的 TypeScript 5.9.3，混淆后的测试脚本结果正确；经典 Rust 混淆工作进程能处理测试项目。
- 测试覆盖远程连接来源、伪造转发头、非法 Host、无效路径/端口、活动操作保护、异常运行态恢复、只取消已提交队列，以及退出完成后拒绝迟到的状态覆盖。原准备参数日志修复及回归测试保留。

可重复的 TUI 验收（临时安装目录中额外安装测试用 node-pty，正式包不依赖它）：

```powershell
npm install --prefix C:\temp\cocos-npm-test --registry=http://192.168.2.33:4873 @tsixi/cocos-build-lan@latest node-pty@1.1.0
node scripts/test-npm-tui.cjs C:\temp\cocos-npm-test
```

## 仍需现场验收

- 另一台真实局域网机器的页面展示及请求限制；当前远程权限验证使用服务端连接来源测试。
- Windows Terminal 中的实际鼠标点击、快捷键打开浏览器、滚轮及直接关闭终端。当前使用伪终端；电脑操作工具明确禁止自动化 Windows Terminal 等终端应用。
- 实际 Cocos 构建、Git 长操作、两种混淆同时处于执行阶段时的逐项退出验收，以及真实只读目录的权限测试。已验证共同的 Job Object 管理、真实准备脚本进程树及状态收尾，但不以此替代每一种任务的现场检查。
- 正常数据目录的首次切换：停止旧控制端/服务，确认无活动任务，再运行 npm 命令。程序会在改写配置前备份旧配置和运行态。
