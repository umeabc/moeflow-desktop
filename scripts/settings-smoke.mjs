// Execute the actual settings page against a local IPC stub. Run with bun -i scripts/settings-smoke.mjs.
import { readFileSync } from 'node:fs';
import assert from 'node:assert/strict';
import { JSDOM } from 'jsdom';

const html = readFileSync(new URL('../src-tauri/ui/settings.html', import.meta.url), 'utf8');
const dom = new JSDOM(html, { url: 'http://127.0.0.1:47199/settings.html', runScripts: 'outside-only' });
const { window } = dom;
const document = window.document;
const calls = [];
let rejectNext = false;
let waitSave;
let releaseSave;
let payload = {
  version: '0.1.1', proxy: { mode: 'system', url: '' },
  active: 'mock', profiles: [{ id: 'mock', name: 'Mock', site_url: 'http://example.test',
    api_base: 'http://example.test/api', port: 47100, allow_invalid_certs: false, media_origins: [] }],
};
window.__TAURI__ = { core: { invoke: async (command, args) => {
  calls.push({ command, args });
  if (command === 'cache_stats') return { entries: 0, bytes: 0, limit_bytes: 2 * 1024 ** 3 };
  if (command === 'boot_payload') return payload;
  if (rejectNext) { rejectNext = false; throw new Error('模拟保存失败'); }
  if (command === 'set_proxy_settings') {
    payload = { ...payload, proxy: args.settings };
    return payload;
  }
  if (command === 'probe_server') return { ok: true, api_base: 'http://example.test/api', message: '连接成功' };
  if (command === 'upsert_profile') {
    if (waitSave) await waitSave;
    payload = { ...payload, profiles: [args.profile] };
    return payload;
  }
  throw new Error(`Unexpected command: ${command}`);
} } };
// Wrap global declarations for Bun's VM compatibility; browser event handlers still use the real DOM.
window.eval(`(function () { ${document.querySelector('script').textContent} })()`);
const tick = () => new Promise(resolve => setTimeout(resolve, 0));
const $ = id => document.getElementById(id);
const mode = name => document.querySelector(`input[name="proxy-mode"][value="${name}"]`);
const select = name => { mode(name).checked = true; mode(name).dispatchEvent(new window.Event('change')); };
await tick();
assert.equal($('version').textContent, 'v0.1.1');
assert.equal(mode('system').checked, true);
assert.equal($('proxy-url-field').hidden, true);
select('manual');
assert.equal($('proxy-url-field').hidden, false);
assert.equal($('proxy-url').disabled, false);
$('proxy-url').value = 'socks5://127.0.0.1:7890';
await $('btn-proxy').onclick();
assert.equal(calls.some(c => c.command === 'set_proxy_settings'), false);
assert.match($('proxy-status').textContent, /HTTP/);
$('proxy-url').value = 'http://127.0.0.1:7890';
await $('btn-proxy').onclick();
assert.deepEqual(JSON.parse(JSON.stringify(calls.findLast(c => c.command === 'set_proxy_settings').args.settings)),
  { mode: 'manual', url: 'http://127.0.0.1:7890' });
assert.match($('proxy-status').textContent, /已保存/);
select('direct');
assert.equal($('proxy-url-field').hidden, true);
rejectNext = true;
await $('btn-proxy').onclick();
assert.match($('proxy-status').textContent, /模拟保存失败/);
assert.equal($('btn-proxy').disabled, false);
assert.equal(mode('direct').checked, true);
// Unrelated server payloads must not overwrite an unsaved proxy change.
document.querySelector('#profiles .row').onclick();
$('f-name').value = 'Edited';
waitSave = new Promise(resolve => { releaseSave = resolve; });
const first = $('btn-save').onclick();
await tick();
assert.equal($('btn-save').disabled, true);
await $('btn-save').onclick();
assert.equal(calls.filter(c => c.command === 'upsert_profile').length, 1);
releaseSave();
await first;
assert.match($('probe-status').textContent, /已保存/);
assert.equal($('btn-save').disabled, false);
assert.equal(mode('direct').checked, true);
rejectNext = true;
await $('btn-save').onclick();
assert.match($('probe-status').textContent, /模拟保存失败/);
assert.equal($('btn-save').disabled, false);
console.log('Settings DOM smoke passed: proxy modes, validation, IPC, version, failures and duplicate-save prevention');
dom.window.close();
