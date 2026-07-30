# LAN Dev 更新源

[English](dev-update.en.md)

`tool-dev-update` 是生成项目内的本地命令，服务于当前平台的**完整版本包**和更新清单。版本包固定包含服务端与控制端二进制；它不广播源代码，也不提供远程控制通道。

```bash
cargo build -p cocos-build-lan-server -p cocos-build-lan-control
cargo run -p cocos-build-lan-core --bin tool-dev-update -- \
  tool.json target/debug/cocos-build-lan-server target/debug/cocos-build-lan-control \
  --listen lan --advertise 192.168.1.24 --broadcast --version 0.1.1-dev.1
```

- 不带 `--listen lan` 时，只监听 `127.0.0.1:49152`。
- `--listen lan` 必须配对 `--advertise <LAN IPv4>`，让其他机器能下载清单内的载荷 URL。
- `--broadcast` 每五秒向 `255.255.255.255:49153` 发送清单提示，且必须搭配 `--listen lan`。
- 接收端必须先在控制端设置页打开并**保存** `ToolSettings.update.lan_dev_enabled`。控制端随后后台监听 UDP 广播，先过滤来源、`tool_id`、平台和包格式；已暂存或已安装的同版本广播会被忽略。

接收端仍会校验文件大小和 SHA-256，之后按普通更新事务暂存。生产安装由 launcher 执行服务安全停止、原子切换、健康检查并启动新版控制端；它无法确认旧控制端退出时不会切换。`cargo run` 开发模式不会替换自身。
