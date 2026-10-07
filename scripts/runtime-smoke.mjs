// Windows WebView2 smoke: real page, IPC and listeners, isolated from user configuration.
// Run after tauri build: bun scripts/runtime-smoke.mjs [path/to/moeflow-desktop.exe]
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { spawn } from 'node:child_process';

const root = mkdtempSync(join(tmpdir(), 'moeflow-011-smoke-'));
const executable = resolve(process.argv[2] ?? new URL('../src-tauri/target/release/moeflow-desktop.exe', import.meta.url).pathname.replace(/^\/(\w:)/, '$1'));
const upstream = Bun.serve({ hostname: '127.0.0.1', port: 0, fetch(req) {
  return new Response(JSON.stringify({ marker: 'first', path: new URL(req.url).pathname }), { headers: { 'content-type': 'application/json' } });
} });
const replacement = Bun.serve({ hostname: '127.0.0.1', port: 0, fetch(req) {
  return new Response(JSON.stringify({ marker: 'second', path: new URL(req.url).pathname }), { headers: { 'content-type': 'application/json' } });
} });
let proxyRequests = 0;
const proxy = Bun.serve({ hostname: '127.0.0.1', port: 0, fetch() {
  proxyRequests++;
  return Response.json({ marker: 'proxy' });
} });
const debugReservation = Bun.serve({ hostname: '127.0.0.1', port: 0, fetch() { return new Response('reserved'); } });
const debugPort = debugReservation.port;
debugReservation.stop(true);
console.log(`Runtime smoke root: ${root}; CDP port: ${debugPort}`);
mkdirSync(join(root, 'config'));
// Deliberately omit proxy: upgrades must default to System.
writeFileSync(join(root, 'config', 'profiles.json'), JSON.stringify({
  active: 'smoke', skip_launcher: false, cache_limit_bytes: 10485760,
  profiles: [{ id: 'smoke', name: 'Smoke', site_url: upstream.url.origin,
    api_base: `${upstream.url.origin}/api`, port: 0, allow_invalid_certs: false, media_origins: [] }],
}));
let child;
let socket;
let stderr = '';
let seq = 0;
const pending = new Map();
const wait = ms => new Promise(resolve => setTimeout(resolve, ms));
async function until(action, label, timeout = 30000) {
  const start = Date.now();
  let error;
  while (Date.now() - start < timeout) {
    try { const value = await action(); if (value) return value; } catch (err) { error = err; }
    if (child && child.exitCode !== null) throw new Error(`App exited ${child.exitCode}: ${stderr}`);
    await wait(150);
  }
  throw new Error(`Timeout: ${label}${error ? ` (${error.message})` : ''}`);
}
async function connect() {
  const pages = await until(async () => {
    const list = await (await fetch(`http://127.0.0.1:${debugPort}/json/list`)).json();
    return list.some(p => /launcher\.html|settings\.html/.test(p.url)) && list;
  }, 'WebView2 debug pages');
  const page = pages.find(p => /launcher\.html|settings\.html/.test(p.url));
  socket = new WebSocket(page.webSocketDebuggerUrl);
  socket.addEventListener('message', event => {
    const message = JSON.parse(event.data);
    const request = pending.get(message.id);
    if (request) { pending.delete(message.id); clearTimeout(request.timer); message.error ? request.reject(new Error(message.error.message)) : request.resolve(message.result); }
  });
  await new Promise((resolve, reject) => {
    socket.addEventListener('open', resolve, { once: true });
    socket.addEventListener('error', reject, { once: true });
  });
  return page;
}
function cdp(method, params = {}) {
  return new Promise((resolve, reject) => {
    const id = ++seq;
    const timer = setTimeout(() => { pending.delete(id); reject(new Error(`${method} timed out`)); }, 15000);
    pending.set(id, { resolve, reject, timer });
    socket.send(JSON.stringify({ id, method, params }));
  });
}
async function evaluate(expression) {
  const result = await cdp('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
  if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text);
  return result.result?.value;
}
async function launch() {
  child = spawn(executable, [], { env: { ...process.env, MOEFLOW_TEST_ROOT: root,
    MOEFLOW_TEST_CDP_PORT: String(debugPort), RUST_BACKTRACE: '1' }, windowsHide: false });
  child.stderr.on('data', data => { stderr += data.toString(); });
  child.on('error', error => { stderr += error.message; });
  const page = await connect();
  await cdp('Page.navigate', { url: new URL('/settings.html', page.url).href });
  await until(() => evaluate('document.getElementById("version")?.textContent === "v0.1.1"'), 'settings initialized');
}
async function stopApp() {
  socket?.close(); socket = null;
  if (child && child.exitCode === null) {
    child.kill();
    await until(() => child.exitCode !== null || child.signalCode !== null, 'test app stopped', 10000);
  }
  child = null;
}
try {
  await launch();
  const initial = await evaluate('window.__TAURI__.core.invoke("boot_payload")');
  assert.equal(initial.version, '0.1.1');
  assert.equal(initial.proxy.mode, 'system');
  const port = initial.active_port;
  assert.ok(port > 0);
  for (let index = 0; index < 3; index++) {
    await evaluate(`(async () => {
      document.querySelector('#profiles .row').click();
      document.getElementById('f-name').value = 'Smoke saved ${index}';
      await document.getElementById('btn-save').onclick();
    })()`);
    assert.equal(await evaluate('document.getElementById("probe-status").textContent'), '已保存');
    assert.equal((await evaluate('window.__TAURI__.core.invoke("boot_payload")')).active_port, port);
    assert.equal((await (await fetch(`http://127.0.0.1:${port}/api/smoke`)).json()).marker, 'first');
    assert.equal(child.exitCode, null);
  }
  await evaluate(`(async () => {
    document.getElementById('f-api').value = ${JSON.stringify(`${replacement.url.origin}/api`)};
    await document.getElementById('btn-save').onclick();
  })()`);
  assert.equal((await (await fetch(`http://127.0.0.1:${port}/api/smoke`)).json()).marker, 'second');
  for (const mode of ['direct', 'manual', 'system']) {
    await evaluate(`(async () => {
      const radio = document.querySelector('input[name="proxy-mode"][value="${mode}"]');
      radio.checked = true; radio.dispatchEvent(new Event('change'));
      document.getElementById('proxy-url').value = ${JSON.stringify(proxy.url.origin)};
      await document.getElementById('btn-proxy').onclick();
    })()`);
    assert.match(await evaluate('document.getElementById("proxy-status").textContent'), /已保存/);
    assert.equal((await evaluate('window.__TAURI__.core.invoke("boot_payload")')).proxy.mode, mode);
    assert.equal((await (await fetch(`http://127.0.0.1:${port}/api/smoke`)).json()).marker, 'second');
  }
  // Point the upstream to a synthetic hostname. Manual mode must send it to the fake proxy.
  await evaluate(`(async () => {
    document.getElementById('f-api').value = 'http://moeflow-smoke.invalid/api';
    await document.getElementById('btn-save').onclick();
    const radio = document.querySelector('input[name="proxy-mode"][value="manual"]');
    radio.checked = true; radio.dispatchEvent(new Event('change'));
    document.getElementById('proxy-url').value = ${JSON.stringify(proxy.url.origin)};
    await document.getElementById('btn-proxy').onclick();
  })()`);
  assert.equal((await (await fetch(`http://127.0.0.1:${port}/api/smoke`)).json()).marker, 'proxy');
  assert.ok(proxyRequests > 0);
  // New profile and deletion must leave the original port/listener intact.
  await evaluate(`(async () => {
    document.getElementById('btn-new').click();
    document.getElementById('f-name').value = 'Smoke extra';
    document.getElementById('f-site').value = ${JSON.stringify(upstream.url.origin)};
    document.getElementById('f-api').value = ${JSON.stringify(`${upstream.url.origin}/api`)};
    await document.getElementById('btn-save').onclick();
  })()`);
  assert.equal((await evaluate('window.__TAURI__.core.invoke("boot_payload")')).profiles.length, 2);
  await evaluate('document.getElementById("btn-delete").onclick()');
  const final = await evaluate('window.__TAURI__.core.invoke("boot_payload")');
  assert.equal(final.profiles.length, 1);
  assert.equal(final.active_port, port);
  assert.equal(child.exitCode, null);
  const saved = JSON.parse(readFileSync(join(root, 'config', 'profiles.json'), 'utf8'));
  assert.equal(saved.proxy.mode, 'manual');
  await stopApp();
  await launch();
  const restored = await evaluate('window.__TAURI__.core.invoke("boot_payload")');
  assert.equal(restored.proxy.mode, 'manual');
  assert.equal(restored.proxy.url.replace(/\/$/, ''), proxy.url.origin);
  assert.equal(restored.active_port, port);
  assert.equal((await (await fetch(`http://127.0.0.1:${port}/api/smoke`)).json()).marker, 'proxy');
  console.log('Windows runtime smoke passed: 3 real saves, hot edit, add/delete, proxy modes, routing, restart persistence');
  console.log(`Isolated diagnostic directory: ${root}`);
} finally {
  await stopApp();
  upstream.stop(true); replacement.stop(true); proxy.stop(true);
}
