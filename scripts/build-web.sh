#!/usr/bin/env bash
set -euo pipefail

project_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$project_root"

pnpm --dir crates/tool-app/editor install --frozen-lockfile
pnpm --dir crates/tool-app/editor run build

# 产物目录固定为 target/dx/cocos-build-lan/release/web/public（dx build 的输出约定）
dx build --release --package cocos-build-lan-app
