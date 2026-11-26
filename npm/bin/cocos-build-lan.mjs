#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { existsSync } from 'node:fs';

if (process.platform !== 'win32' || process.arch !== 'x64') {
  console.error('cocos-build-lan 目前仅支持 Windows x64。');
  process.exit(1);
}
const executable = fileURLToPath(new URL('../runtime/cocos-build-lan-server.exe', import.meta.url));
if (!existsSync(executable)) {
  console.error('npm 包缺少服务程序，请重新安装完整发布包。');
  process.exit(1);
}
// Register before starting Rust; Node must not change console handling after TUI setup.
process.on('SIGINT', () => {});
process.on('SIGBREAK', () => {});
const child = spawnSync(executable, process.argv.slice(2), {
  stdio: 'inherit',
  env: { ...process.env, COCOS_BUILD_LAN_NODE_EXE: process.execPath },
});
// Windows delivers console signals to both processes. Rust owns confirmation/cleanup.
if (child.error) console.error(`启动失败：${child.error.message}`);
process.exitCode = child.status ?? 1;
