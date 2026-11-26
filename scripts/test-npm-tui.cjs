// npm install --prefix <test-install> @tsixi/cocos-build-lan@next node-pty@1.1.0
// node scripts/test-npm-tui.cjs <test-install>
const assert = require('node:assert/strict');
const fs = require('node:fs');
const net = require('node:net');
const os = require('node:os');
const path = require('node:path');
const { createRequire } = require('node:module');

const install = path.resolve(process.argv[2]);
const pty = createRequire(path.join(install, 'package.json'))('node-pty');
const entry = path.join(install, 'node_modules/@tsixi/cocos-build-lan/bin/cocos-build-lan.mjs');
const version = JSON.parse(fs.readFileSync(path.join(install, 'node_modules/@tsixi/cocos-build-lan/package.json'))).version;
const data = fs.mkdtempSync(path.join(os.tmpdir(), 'cocos-tui-'));
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function wait(predicate, label) {
  for (let i = 0; i < 150; i++) {
    if (await predicate()) return;
    await delay(100);
  }
  throw new Error(`${label} timed out`);
}
async function freePort() {
  const socket = net.createServer();
  await new Promise(resolve => socket.listen(0, '127.0.0.1', resolve));
  const port = socket.address().port;
  await new Promise(resolve => socket.close(resolve));
  return port;
}
let active;
async function run(headless, port) {
  let screen = '', exit;
  const child = pty.spawn(process.execPath, [entry, ...(headless ? ['--no-tui'] : [])], {
    cols: 120, rows: 35, cwd: data,
    env: { ...process.env, RUST_LOG: 'info', COCOS_BUILD_LAN_DATA_ROOT: data },
  });
  active = child;
  child.onData(value => { screen += value; });
  child.onExit(value => { exit = value; active = null; });
  const base = `http://127.0.0.1:${port}`;
  await wait(async () => { try { return (await fetch(`${base}/healthz`)).ok; } catch { return false; } }, 'service');
  assert.equal((await (await fetch(`${base}/healthz`)).json()).version, version);
  assert.equal((await (await fetch(`${base}/api/access-context`)).json()).canManageMachine, true);
  if (!headless) {
    await wait(() => screen.includes('Q/Ctrl+C'), 'TUI');
    screen = '';
    child.write('q');
    await wait(() => screen.includes('退出确认'), 'Q confirmation');
    child.write('\r');
    await delay(200);
    assert.equal((await fetch(`${base}/healthz`)).status, 200);
    screen = '';
    child.write(' ');
    await wait(() => screen.includes('已暂停'), 'pause logs');
    child.write('\x1b[F');
    screen = '';
    child.write('\x03');
    await wait(() => screen.includes('退出确认'), 'Ctrl+C confirmation');
    child.write('\x1b[C');
    await delay(200);
    child.write('\r');
  } else {
    child.write('\x03');
  }
  await wait(() => exit, 'clean exit');
  assert.equal(exit.exitCode, 0);
  assert.match(fs.readFileSync(path.join(data, 'logs/server.log'), 'utf8'), /全部任务已停止/);
  assert.equal(fs.existsSync(path.join(data, 'instance.json')), false);
}
(async () => {
  const port = await freePort();
  fs.mkdirSync(path.join(data, 'config'));
  fs.writeFileSync(path.join(data, 'config/tool-settings.json'), JSON.stringify({ network: { lan_port: port }, business: {} }));
  await run(false, port);
  await run(true, port);
  console.log(JSON.stringify({ version, data, passed: ['TUI', 'Q default cancel', 'log pause', 'Ctrl+C confirmation', 'headless Ctrl+C', 'persisted shutdown'] }));
})().then(() => process.exit(0), error => {
  console.error(error);
  if (active) active.kill();
  process.exit(1);
});
