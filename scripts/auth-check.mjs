#!/usr/bin/env node
// Login checks against a server with a password: the page redirects, every API
// call needs a session, the cookie is HttpOnly/Secure/SameSite=Strict, failed
// logins are limited, and a browser on 127.0.0.1 can really log in.
//   AUTH_BASE=http://127.0.0.1:PORT node scripts/auth-check.mjs

import { spawn } from 'node:child_process';
import { mkdtempSync } from 'node:fs';
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
const port = 9900 + Math.floor(Math.random() * 90);
const profile = mkdtempSync(join(tmpdir(), 'herdr-auth-'));
const chrome = spawn(process.env.CHROMIUM || 'chromium', [
  '--headless=new', '--no-sandbox', '--disable-gpu', '--hide-scrollbars', `--user-data-dir=${profile}`,
  `--remote-debugging-port=${port}`, '--window-size=1200,850',
  '--host-resolver-rules=MAP remote.test 127.0.0.1', 'about:blank',
], { stdio: 'ignore' });
let ws;
const pending = new Map(); let id = 0;
const send = (method, params = {}) => new Promise((r) => { const i = ++id; pending.set(i, r); ws.send(JSON.stringify({ id: i, method, params })); });
const evaluate = async (expr) => (await send('Runtime.evaluate', { expression: expr, returnByValue: true, awaitPromise: true })).result?.result?.value;
for (let i = 0; i < 60; i++) {
  try { const r = await fetch(`http://127.0.0.1:${port}/json/version`); if (r.ok) break; } catch {}
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
  if (await evaluate('location.pathname === "/" && !!document.querySelector("#hud") && !!window.world && world.debug().planets > 0')) { loggedIn = true; break; }
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
const remoteAuthed = await get('/api/limits', cookie, { headers: { 'x-forwarded-for': '203.0.113.5' } });
check('a logged-in remote caller reads limits', remoteAuthed.status === 200, String(remoteAuthed.status));

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
