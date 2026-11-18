import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { resolve } from 'node:path';
const port = 17623;
const base = 'http://127.0.0.1:' + port;
const teacher = 'teacher-test-credential-1234567890';
const pupil = 'pupil-test-credential-0987654321';
const child = spawn(resolve('target/release/oko-master'), ['--port', String(port)], {
  env: { ...process.env, OKO_TOKEN: teacher, OKO_AGENT_TOKEN: pupil },
  stdio: 'ignore',
});
const pause = ms => new Promise(r => setTimeout(r, ms));
async function http(path, key) {
  return fetch(base + path, { headers: key ? { Authorization: 'Bearer ' + key } : {} });
}
async function ws(path, key) {
  const url = 'ws://127.0.0.1:' + port + path + (key ? '?token=' + encodeURIComponent(key) : '');
  return new Promise(resolve => {
    const socket = new WebSocket(url);
    let settled = false;
    function done(value) {
      if (settled) return;
      settled = true;
      try { socket.close(); } catch {}
      resolve(value);
    }
    socket.addEventListener('open', () => done(true), { once: true });
    socket.addEventListener('error', () => done(false), { once: true });
    setTimeout(() => done(false), 2000);
  });
}
try {
  await pause(600);
  assert.equal((await http('/healthz')).status, 200);
  assert.equal((await http('/api/state')).status, 401);
  assert.equal((await http('/api/state', pupil)).status, 401);
  assert.equal((await http('/api/state', teacher)).status, 200);
  assert.equal(await ws('/ws/console', pupil), false);
  assert.equal(await ws('/ws/console', teacher), true);
  assert.equal(await ws('/ws/agent', teacher), false);
  assert.equal(await ws('/ws/agent', pupil), true);
  console.log('PASS: role isolation 8/8');
} finally {
  child.kill('SIGTERM');
}
process.exit(0);
