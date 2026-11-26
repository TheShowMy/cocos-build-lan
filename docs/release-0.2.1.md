# 0.2.1 发布记录

日期：2026-10-13；Windows x64。

- 包：`@tsixi/cocos-build-lan@0.2.1`，内网仓库 `http://192.168.2.33:4873`。
- 先发布 `next`，从全新 npm 缓存冷启动验收，再将相同完整性的包提升到 `latest`。最终两个标签均为 `0.2.1`。
- 本地产物：`dist/tsixi-cocos-build-lan-0.2.1.tgz`；5,199,566 字节，11 个文件。
- 完整性：`sha512-fHQy+IlFN+41tvFzZwJvfFBdOBpWtviFhTvTqq33RR6+Tg+/1ZSk8TVJ34nuCaU7NMpOXiZCPLs0FEnMbySzjw==`。

## 本次内容

英文标识参数、小写保存及统一占位符；动态说明与 favicon；分支缓存、任务组/勾选记忆；常驻组内操作及全局队列；未保存保护和队列取消修复。保留现有 npm/TUI 架构，完整功能验收见 [Web 打包页验收](web-package-acceptance.md)。

## 发布验证

- 版本调整后格式检查、工作区 122 项测试、编译检查、Clippy、差异检查通过；Web 和 Rust release 重新构建。
- 检查包版本、必要资源、体积及运行数据排除。只装入当前入口引用的 6 个 Web 文件，不包含历史哈希资源。
- 使用 `@next` 从新 npm 缓存启动；健康检查和包元数据均为 `0.2.1`，运行程序路径确实来自该缓存。
- HTTP 读取的 6 个 Web 文件逐字节匹配发布产物；包内混淆脚本可加载 TypeScript `5.9.3`。
- 沿用上一轮隔离验收数据，启动前后配置、项目/任务组设置和准备脚本哈希一致。没有触碰正式数据目录。
- 提升 `latest` 前比较远程与本地产物 SHA-512；提升后 `npx ... @latest --version` 返回 `cocos-build-lan 0.2.1`。
- 临时验收服务已停止。真实 Cocos 构建和另一台机器的现场测试由用户继续进行。

## 真机启动

先正常退出旧实例，再执行：

```powershell
npx --yes --prefer-online --registry=http://192.168.2.33:4873 @tsixi/cocos-build-lan@latest
```

确认 Web 顶部显示 `v0.2.1`。原配置与业务数据沿用，不要为正常升级设置新的 `COCOS_BUILD_LAN_DATA_ROOT`。
