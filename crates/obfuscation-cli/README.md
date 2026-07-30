# obfuscation-cli

用于 Cocos JS 混淆的 Rust 命令行工具。

## 快速开始

```bash
cargo run -- run \
  --input-dir ./build/src \
  --output-dir ./build/src-obf \
  --assets-dir ./assets \
  --dict-dir ./python/obfuscation \
  --build-dir ./build \
  --seed 42
```

默认会输出映射文件，路径为当前工作目录下的 `./obfuscation_mapping.json`。
白名单会统一输出到 `dict-dir/whitelist.json`。

## 子命令

- `run`
- `refresh-exclude`
- `extract-engine`
- `doctor`
