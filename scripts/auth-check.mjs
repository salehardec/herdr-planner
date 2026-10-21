#!/usr/bin/env node
// Login checks against a server with a password: the page redirects, every API
// call needs a session, the cookie is HttpOnly/Secure/SameSite=Strict, failed
// logins are limited, and a browser on 127.0.0.1 can really log in.
//   AUTH_BASE=http://127.0.0.1:PORT node scripts/auth-check.mjs

import { spawn } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const base = process.env.AUTH_BASE || process.argv[2];
if (!base) { console.error('auth-check: AUTH_BASE is required'); process.exit(2); }
let failed = 0;
const check = (name, ok, extra = '') => {
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${name}${extra ? ' — ' + extra : ''}`);
  if (!ok) failed++;
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const get = (path, cookie, opts = {}) => fetch(base + path, { redirect: 'manual', ...opts, headers: { ...(cookie ? { cookie } : {}), ...(opts.headers || {}) } });
const json = async (r) => ({ status: r.status, body: await r.json().catch(() => null), headers: r.headers });

const noAuth = await get('/api/state');
check('API without a session is 401', noAuth.status === 401, String(noAuth.status));
for (const path of ['/api/events', '/api/search?q=alpha', '/api/models?program=claude', '/api/subagent?task=quasar&id=x', '/api/task/closing?id=alpha/1', '/api/fs/complete?path=/', '/api/universe/info?repo=alpha', '/api/chat/history?q=x']) {
  const r = await get(path);
  check(`${path.split('?')[0]} needs a session`, r.status === 401, String(r.status));
}
for (const [path, body] of [['/api/translate', { text: 'hello' }], ['/api/galaxy/ask', { label: 'vm-a' }], ['/api/galaxy/add', { target: 'a@b', confirm: true }], ['/api/universe/inspect', { input: '/tmp' }], ['/api/universe/add', { mode: 'folder', name: 'x', path: '/tmp', confirm: true }], ['/api/universe/fork', { repo: 'alpha', op: 'pull', confirm: true }]]) {
  const r = await get(path, null, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) });
  check(`POST ${path} needs a session`, r.status === 401, String(r.status));
}
const localLimits = await get('/api/limits');
check('the local bar can read limits without a session', localLimits.status === 200 && (await localLimits.json()).level === 'ok', String(localLimits.status));
const remoteLimits = await get('/api/limits', null, { headers: { 'x-forwarded-for': '203.0.113.5' } });
check('a remote caller needs a session for limits', remoteLimits.status === 401, String(remoteLimits.status));
const root = await get('/');
check('the page redirects to /login', root.status >= 300 && root.status < 400 && (root.headers.get('location') || '').includes('/login'), `${root.status} ${root.headers.get('location')}`);
const loginPage = await get('/login');
const loginText = await loginPage.text();
check('the login page carries the Omarchy theme', loginPage.status === 200 && loginText.includes('--c-accent') && loginText.includes('__THEME__') === false);

const wrong = await json(await fetch(base + '/api/login', {
  method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ user: 'me', password: 'wrong' }),
}));
check('a wrong password is refused', wrong.status === 401);

// Browser login first, before the per-address limit is reached.
const profile = mkdtempSync(join(tmpdir(), 'herdr-auth-'));
// The browser picks its own debugging port (several checks run side by side; a random port could be taken twice):
// it writes the port into its profile folder.
let port = 0;
const readPort = () => { try { return Number(readFileSync(join(profile, 'DevToolsActivePort'), 'utf8').split('\n')[0]) || 0; } catch { return 0; } };
const chrome = spawn(process.env.CHROMIUM || 'chromium', [
  '--headless=new', '--no-sandbox', '--disable-gpu', '--hide-scrollbars', `--user-data-dir=${profile}`,
  '--remote-debugging-port=0', '--window-size=1200,850',
  '--host-resolver-rules=MAP remote.test 127.0.0.1', 'about:blank',
], { stdio: 'ignore' });
// Whatever happens, the browser goes and its profile (hundreds of MB in /tmp) goes with it.
process.on('exit', () => {
  try { chrome.kill('SIGKILL'); } catch { /* gone already */ }
  try { rmSync(profile, { recursive: true, force: true, maxRetries: 3 }); } catch { /* best effort */ }
  // the killed browser may still write a last file into the folder: sweep once more a moment later, detached
  try { spawn('sh', ['-c', `sleep 2; rm -rf '${profile}'`], { detached: true, stdio: 'ignore' }).unref(); } catch { /* best effort */ }
});
let ws;
const pending = new Map(); let id = 0;
// A browser that stopped answering (a crashed renderer) would leave the check waiting for ever: when the latest call
// has had no reply for 120 s, say so and stop. (A call somebody gave up on earlier does not count: later ones are answered.)
let lastSentAt = 0; let lastRepliedAt = 0;
setInterval(() => {
  if (lastSentAt > lastRepliedAt && Date.now() - lastSentAt > 120000) { console.log('FAIL the browser stopped answering'); process.exit(1); }
}, 5000).unref();
const send = (method, params = {}) => new Promise((r) => {
  const i = ++id;
  lastSentAt = Date.now();
  pending.set(i, (m) => { lastRepliedAt = Date.now(); r(m); });
  ws.send(JSON.stringify({ id: i, method, params }));
});
const evaluate = async (expr) => (await send('Runtime.evaluate', { expression: expr, returnByValue: true, awaitPromise: true })).result?.result?.value;
for (let i = 0; i < 60; i++) {
  port = port || readPort();
  try { if (port) { const r = await fetch(`http://127.0.0.1:${port}/json/version`); if (r.ok) break; } } catch {}
  await sleep(200);
}
const tabs = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
ws = new WebSocket(tabs.find((t) => t.type === 'page').webSocketDebuggerUrl);
await new Promise((r) => { ws.onopen = r; });
ws.onmessage = (e) => { const m = JSON.parse(e.data); if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); } };
await send('Runtime.enable');
await send('Page.enable');
await send('Page.navigate', { url: base + '/' });
await sleep(900);
check('the browser lands on the login form', await evaluate('location.pathname === "/login" && !!document.querySelector("#user")'));
check('the post-login next stays on this site', await evaluate("typeof safeNext === 'function' && safeNext('/' + String.fromCharCode(92) + 'evil.example/x') === '/' && safeNext('//evil.example') === '/' && safeNext('/tasks?x=1') === '/tasks?x=1'"), await evaluate("typeof safeNext === 'function' ? safeNext('/' + String.fromCharCode(92) + 'evil.example/x') : 'missing safeNext'"));

// A remote client on plain HTTP must be told to use HTTPS before its
// credentials are sent; the Secure cookie would otherwise be dropped.
await send('Page.navigate', { url: `http://remote.test:${new URL(base).port}/login` });
await sleep(800);
await evaluate('document.querySelector("#user").value = "me"; document.querySelector("#pass").value = "secret"; document.querySelector("#login").requestSubmit()');
await sleep(500);
check('remote plain HTTP login is refused with a clear message', await evaluate('location.hostname === "remote.test" && location.pathname === "/login" && document.querySelector("#err").textContent.includes("HTTPS")'), await evaluate('document.querySelector("#err").textContent'));
await send('Page.navigate', { url: base + '/' });
await sleep(900);
await evaluate('document.querySelector("#user").value = "me"; document.querySelector("#pass").value = "secret"; document.querySelector("#login").requestSubmit()');
let loggedIn = false;
// The 3D world compiles its shaders under SwiftShader; login plus the first
// frame can take longer than the old flat page.
for (let i = 0; i < 150; i++) {
  if (await evaluate('location.pathname === "/" && !!document.querySelector("#top") && !!window.world && world.debug().planets > 0')) { loggedIn = true; break; }
  await sleep(200);
}
check('the browser logs in and sees the dashboard', loggedIn);
check('the session cookie is HttpOnly', (await evaluate('document.cookie')) === '', await evaluate('document.cookie'));
ws.close();
chrome.kill();

// Now the limit: four more wrong attempts, then a correct one, then five wrong.
for (let i = 0; i < 4; i++) await fetch(base + '/api/login', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ user: 'me', password: 'wrong' }) });
const good = await fetch(base + '/api/login', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ user: 'me', password: 'secret' }) });
const setCookie = good.headers.get('set-cookie') || '';
check('a correct login sets the session cookie', good.status === 200 && setCookie.includes('herdr_planner_session='), `${good.status}`);
for (const attr of ['HttpOnly', 'Secure', 'SameSite=Strict', 'Path=/']) check(`the cookie has ${attr}`, setCookie.includes(attr), setCookie);
const cookie = setCookie.split(';')[0];
const authed = await get('/api/state', cookie);
check('the session opens the API', authed.status === 200, String(authed.status));
const stream = await fetch(base + '/api/events', { headers: { cookie } });
check('the event stream opens with a session and starts with the state', stream.status === 200 && (stream.headers.get('content-type') || '').includes('text/event-stream'), `${stream.status} ${stream.headers.get('content-type')}`);
const reader = stream.body.getReader();
const firstChunk = await reader.read();
check('its first event is the whole snapshot', new TextDecoder().decode(firstChunk.value).startsWith('event: state'), new TextDecoder().decode(firstChunk.value).slice(0, 40));
await reader.cancel();
const assetsAnon = await get('/assets/nothing.js');
check('the built assets are not behind a secret (public, hashed files)', assetsAnon.status === 404 || assetsAnon.status === 200, String(assetsAnon.status));
const remoteAuthed = await get('/api/limits', cookie, { headers: { 'x-forwarded-for': '203.0.113.5' } });
check('a logged-in remote caller reads limits', remoteAuthed.status === 200, String(remoteAuthed.status));

// Images for the Coordinator: behind the login, type by bytes, size limit,
// and the full path (not the browser's path) in the message.
const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAFgwJ/lK3Q6wAAAABJRU5ErkJggg==', 'base64');
const form = (bytes, name, type) => {
  const fd = new FormData();
  fd.append('file', new Blob([bytes], { type }), name);
  return fd;
};
// A server that refuses a request (401, 413…) at once, without reading the body, closes the connection; a client still
// writing then sees a reset (EPIPE / ECONNRESET) instead of the status. The form is sent as one buffer with its length
// (written in one go), a reset is tried again, and for a request that is *meant* to be refused a reset that stays counts as
// the refusal (`ifReset` is the status it stands for).
const reset = (e) => ['EPIPE', 'ECONNRESET', 'UND_ERR_SOCKET'].includes(e?.cause?.code);
const upload = async (body, withCookie = true, ifReset = null) => {
  const packed = new Response(body);
  const bytes = Buffer.from(await packed.arrayBuffer());
  const headers = { 'content-type': packed.headers.get('content-type') || '', ...(withCookie ? { cookie } : {}) };
  for (let i = 0; ; i++) {
    try {
      return await fetch(base + '/api/chat/attach', { method: 'POST', headers, body: bytes });
    } catch (e) {
      if (!reset(e)) throw e;
      if (i >= 2) { if (ifReset) return { status: ifReset }; throw e; }
    }
  }
};
const anonymous = await upload(form(png, 'shot.png', 'image/png'), false, 401);
check('an upload without a session is 401', anonymous.status === 401, String(anonymous.status));
const uploaded = await upload(form(png, 'shot.png', 'image/png'));
const uploadedBody = await uploaded.json().catch(() => ({}));
check('a PNG upload is accepted', uploaded.status === 200 && uploadedBody.attachment?.mime === 'image/png', `${uploaded.status} ${JSON.stringify(uploadedBody)}`);
const imageId = uploadedBody.attachment?.id || '';
check('the upload gets a generated safe name', /^[0-9a-f]{32}\.png$/.test(imageId), imageId);
const imageAnon = await fetch(base + `/api/attachments/${imageId}`);
check('the image file is behind the login too', imageAnon.status === 401, String(imageAnon.status));
const image = await fetch(base + `/api/attachments/${imageId}`, { headers: { cookie } });
check('the uploaded image loads with its type', image.status === 200 && image.headers.get('content-type') === 'image/png', `${image.status} ${image.headers.get('content-type')}`);
check('the served image is the uploaded one', Buffer.from(await image.arrayBuffer()).equals(png));
const html = await upload(form(Buffer.from('<html>not an image</html>'), 'fake.png', 'image/png'));
check('a file that is not really an image is refused', html.status === 415, String(html.status));
const empty = await upload(new FormData());
check('an upload without a file is refused', empty.status === 400, String(empty.status));
const huge = Buffer.alloc(10 * 1024 * 1024 + 1, 0);
const oversized = await upload(form(huge, 'big.png', 'image/png'), true, 413);
check('an image over 10 MB is refused', oversized.status === 413, String(oversized.status));
const overBody = await upload(form(Buffer.alloc(11 * 1024 * 1024 + 1, 0), 'huge.png', 'image/png'), true, 413);
check('a body over the request limit is 413 too', overBody.status === 413, String(overBody.status));

const chat = await fetch(base + '/api/chat', {
  method: 'POST',
  headers: { 'content-type': 'application/json', cookie },
  body: JSON.stringify({ text: 'посмотри картинку', attachments: [imageId] }),
});
check('a message with an image reaches the Coordinator', chat.status === 200, String(chat.status));
const fixture = process.env.FIXTURE;
if (fixture) {
  let history = null;
  for (let i = 0; i < 40; i++) {
    const s = await (await fetch(base + '/api/state', { headers: { cookie } })).json();
    history = s.coordinator?.history?.at(-1);
    if (history?.attachments?.length === 1) break;
    await sleep(150);
  }
  check('the chat history keeps the image with the message', history?.text === 'посмотри картинку' && history?.attachments?.length === 1, JSON.stringify(history));
  const tells = readFileSync(join(fixture, 'log', 'coordinator.jsonl'), 'utf8').trim().split('\n').map((l) => JSON.parse(l));
  const tell = tells.at(-1);
  const path = tell.text.split('\n').find((l) => l.startsWith(join(fixture, 'state-auth', 'herdr-planner', 'attachments') + '/'));
  check('the Coordinator gets the absolute path on this machine', tell.text.includes('посмотри картинку') && !!path, tell.text.replace(/\n/g, ' | '));
  check('the path in the message exists', path && existsSync(path));
} else {
  console.log('skip the path-in-message check: FIXTURE is not set');
}

for (let i = 0; i < 5; i++) await fetch(base + '/api/login', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ user: 'me', password: 'wrong' }) });
const limited = await json(await fetch(base + '/api/login', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ user: 'me', password: 'secret' }) }));
check('failed logins are limited', limited.status === 429, String(limited.status));
const still = await get('/api/state', cookie);
check('an existing session survives the limit', still.status === 200, String(still.status));

const out = await fetch(base + '/api/logout', { method: 'POST', headers: { cookie } });
check('logout answers', out.status === 200);
const after = await get('/api/state', cookie);
check('the session is gone after logout', after.status === 401, String(after.status));

process.exit(failed ? 1 : 0);
