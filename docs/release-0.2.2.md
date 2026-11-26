# 0.2.2 发布记录

日期：2026-10-13；Windows x64。

- 包：`@tsixi/cocos-build-lan@0.2.2`，仓库 `http://192.168.2.33:4873`。
- 先发布 `next`，全新缓存冷启动检查通过后，将同一包提升为 `latest`；两个标签最终均为 `0.2.2`。
- 产物：`dist/tsixi-cocos-build-lan-0.2.2.tgz`，5,375,787 字节，11 个文件。
- 完整性：`sha512-y+QCFkS4swnKVrswiCoqza1uOUPcM8nk3S0qGvNtSH/TjsQ95QWLIgD1MV+mEY5ZAdS7ieaUrSZ5h0EzHYEnHw==`。

## 本次内容

左右独立滚动；构建方案管理、默认关联、勾选自动保存及参数同步；内置版本参数和统一前缀；保存提示计时；紧凑参数说明浮层；移除重复页面标题，将刷新、新建任务归入列表操作栏。

## 验证

- 版本更新后格式、136 项工作区测试、编译、Clippy、差异检查通过，Web 和 Rust release 重新构建。
- 发布脚本校验版本一致性、必要资源、包体积和运行数据排除；Web 输出先清理后构建，无旧哈希资源。
- 从空工作目录使用 `npx ... @next --no-tui` 启动，使用全新 npm 缓存和独立数据目录；服务程序确实来自新缓存，健康检查返回 `0.2.2`。
- HTTP 返回的 6 个 Web 文件与包内产物逐字节一致，混淆脚本可解析 TypeScript `5.9.3`。
- 8 个业务配置及准备脚本文件哈希保持一致；本机配置经启动时格式化写回后，所有字段值保持一致。
- 提升 `latest` 前，脚本核对远程包与本地产物 SHA-512 一致；提升后 `npx ... @latest --version` 返回 `cocos-build-lan 0.2.2`。
- 冷启动临时服务已停止，正式数据目录未使用。本次未重新执行真实 Cocos 构建或 TUI 交互验收。

验证日志为 `target/release-0.2.2-*.log`，冷启动结果为 `target/release-0.2.2-smoke.json`。功能界面验收见 [紧凑面板](compact-task-panel-acceptance.md) 和 [列表操作栏](task-toolbar-acceptance.md)。

## 启动

正常退出旧实例后执行：

```powershell
npx --yes --prefer-online --registry=http://192.168.2.33:4873 @tsixi/cocos-build-lan@latest
```

Web 顶部应显示 `v0.2.2`。正常升级沿用原数据目录，不设置新的 `COCOS_BUILD_LAN_DATA_ROOT`。
