#!/usr/bin/env node
// The gate (Шлюз) from the outside, on the real binary and the stand-in of cloudflared (scripts/fake-cloudflared.mjs, the
// fixture's bin/cloudflared): open the tunnel, come in with the link's key as a phone, let it in, raise a computer with the
// password, try every door that must stay shut, switch a device off, close the gate. The «internet» is this script: a
// request with the headers Cloudflare adds (Cf-*, the tunnel's address in Host) is one that came through the tunnel.
// Runs on the server with a password (user me, password secret), because the full version needs one.
//   AUTH_BASE=http://127.0.0.1:PORT node scripts/tunnel-check.mjs
//   (scripts/run-checks.sh runs it in the part «auth»; the same checks in a browser: the sections «gate» and «gate page» of ui-check)
import http from 'node:http';

const base = process.env.AUTH_BASE || process.argv[2];
if (!base) { console.error('tunnel-check: AUTH_BASE is required'); process.exit(2); }
const target = new URL(base);
const HOST = 'fake-quick-tunnel.trycloudflare.com';
const IPHONE = 'Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1';
const LINUX = 'Mozilla/5.0 (X11; Linux x86_64; rv:125.0) Gecko/20100101 Firefox/125.0';
const USER = 'me';
const PASSWORD = 'secret';

let failed = 0;
const check = (name, ok, extra = '') => {
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${name}${extra ? ' — ' + extra : ''}`);
  if (!ok) failed++;
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** One request on a plain socket (fetch would not let us write Host). */
function raw(method, path, headers = {}, body) {
  return new Promise((resolve, reject) => {
    const payload = body === undefined ? undefined : typeof body === 'string' ? body : JSON.stringify(body);
    const h = { connection: 'close', ...headers };
    if (payload !== undefined) { h['content-type'] = 'application/json'; h['content-length'] = Buffer.byteLength(payload); }
    const req = http.request({ host: target.hostname, port: target.port, method, path, headers: h, timeout: 15000 }, (res) => {
      const chunks = [];
      res.on('data', (c) => chunks.push(c));
      res.on('end', () => {
        const text = Buffer.concat(chunks).toString('utf8');
        let json = null;
        try { json = JSON.parse(text); } catch { /* not JSON */ }
        resolve({ status: res.statusCode, headers: res.headers, text, json, cookies: [].concat(res.headers['set-cookie'] || []) });
      });
    });
    req.on('timeout', () => req.destroy(new Error(`${method} ${path}: no answer`)));
    req.on('error', reject);
    if (payload !== undefined) req.write(payload);
    req.end();
  });
}

/** A request as it arrives through the tunnel: what Cloudflare adds, the tunnel's name as Host, the caller's cookies. */
const tun = (method, path, { cookie = '', ua = LINUX, headers = {}, body } = {}) => raw(method, path, {
  host: HOST, 'cf-ray': 'tunnel-check', 'cf-visitor': '{"scheme":"https"}', 'cf-connecting-ip': '203.0.113.9', 'cf-ipcountry': 'RU', 'user-agent': ua,
  ...(cookie ? { cookie } : {}), ...headers,
}, body);

// the owner, at this machine: the planner's own login first
let ownerCookie = '';
const own = (method, path, body) => raw(method, path, { host: target.host, origin: base, ...(ownerCookie ? { cookie: ownerCookie } : {}) }, body);
const cookieOf = (res, name) => {
  const line = res.cookies.find((c) => c.toLowerCase().startsWith(name.toLowerCase() + '='));
  return line ? line.split(';')[0] : '';
};
const view = async () => (await own('GET', '/api/tunnel')).json;
const waitView = async (pred, ms = 15000) => {
  const end = Date.now() + ms;
  let v = null;
  while (Date.now() < end) { v = await view(); if (v && pred(v)) return v; await sleep(100); }
  return v;
};
const keyOf = (v) => (v?.link || '').split('#k=')[1] || '';
const redeem = (key, ua, extra = {}) => tun('POST', '/g/redeem', { ua, body: { key, touch: ua === IPHONE }, ...extra });

/** Where a place of this machine hides in a snapshot: a key naming one, with something in it. */
const PLACES = ['worktree', 'session', 'sessionFile', 'sessionPath', 'cwd', 'reportsDir', 'reports_dir', 'transcript'];
const places = (v, at = '') => {
  if (Array.isArray(v)) return v.flatMap((x, i) => places(x, `${at}[${i}]`));
  if (v && typeof v === 'object') return Object.entries(v).flatMap(([k, x]) => (PLACES.includes(k) && x ? [`${at}.${k}`] : places(x, `${at}.${k}`)));
  return [];
};
const brief = (res) => `${res.status} ${res.text.slice(0, 120).replace(/\s+/g, ' ')}`;

try {
  // ---- the owner logs in; the gate is closed
  const login = await raw('POST', '/api/login', { host: target.host, origin: base }, { user: USER, password: PASSWORD });
  ownerCookie = cookieOf(login, 'herdr_planner_session');
  check('the owner logs in at this machine', login.status === 200 && !!ownerCookie, brief(login));
  const closed0 = await view();
  check('the gate starts closed', closed0?.state === 'closed' && closed0?.link === null, JSON.stringify(closed0?.state));

  const page = { accept: 'text/html' };
  for (const [method, path] of [['GET', '/'], ['GET', '/api/state'], ['POST', '/g/redeem'], ['GET', '/m'], ['GET', '/api/tunnel'], ['GET', '/nope']]) {
    const r = await tun(method, path, { headers: path === '/' ? page : {}, body: method === 'POST' ? {} : undefined });
    check(`closed: ${method} ${path} through the tunnel is 503`, r.status === 503, brief(r));
  }
  const bare = await raw('GET', '/', { host: HOST, accept: 'text/html' });
  check('closed: the tunnel\'s name alone (no Cf-* header) is the tunnel\'s too: 503, a page of words, no script', bare.status === 503 && /<h1>/.test(bare.text) && !/<script/i.test(bare.text), brief(bare));
  const rebound = await raw('GET', '/api/state', { host: 'attacker.example:7117', origin: 'http://attacker.example:7117', cookie: ownerCookie });
  check('a page of another site whose name points here gets 421, even with the owner\'s cookie', rebound.status === 421, brief(rebound));
  const forgedClosed = await tun('GET', '/api/tunnel', { headers: { host: 'localhost', 'x-forwarded-for': '127.0.0.1', 'cf-connecting-ip': '127.0.0.1' }, cookie: ownerCookie });
  check('closed: Host localhost and X-Forwarded-For 127.0.0.1 on a request with Cf-* headers change nothing: 503', forgedClosed.status === 503, brief(forgedClosed));

  // ---- open
  const opened = await own('POST', '/api/tunnel/open', {});
  check('the owner opens the gate', opened.status === 200, brief(opened));
  const v1 = await waitView((v) => v.state === 'open');
  const key = keyOf(v1);
  check('open: the stand-in\'s address and a link with a 22-sign key', v1?.state === 'open' && v1.host === HOST && /^[\w-]{22}$/.test(key), JSON.stringify({ state: v1?.state, host: v1?.host }));

  // ---- a stranger: the sign-in page and nothing
  const front = await tun('GET', '/', { headers: page });
  check('a stranger gets the sign-in page and its script', front.status === 200 && front.text.includes('/g/gate.js') && !front.text.includes(key), brief(front));
  const shut = [['GET', '/api/state'], ['GET', '/api/events'], ['POST', '/api/action'], ['POST', '/api/chat'], ['GET', '/m/api/state'], ['GET', '/m/api/poll'], ['POST', '/m/api/action'], ['GET', '/assets/x.js'], ['GET', '/login']];
  const hidden = [['GET', '/api/limits'], ['GET', '/api/tunnel'], ['POST', '/api/tunnel/open'], ['POST', '/api/tunnel/close'], ['POST', '/api/tunnel/rotate'], ['POST', '/api/tunnel/session'], ['POST', '/api/tunnel/settings'],
    ['POST', '/api/task/terminal'], ['POST', '/api/runner/terminal'], ['POST', '/api/runner/container/shell'], ['GET', '/api/fs/complete'], ['GET', '/api/hints/files'], ['POST', '/api/runner/add'], ['POST', '/api/station/set'],
    ['POST', '/api/universe/add'], ['POST', '/api/memory/save'], ['POST', '/api/translate'], ['POST', '/api/ship'], ['GET', '/nope']];
  // what a caller gets at every door of a list: the ones that did not answer `want`
  const denied = async (list, want, who = {}, forged = {}) => {
    const wrong = [];
    for (const [m, p] of list) {
      const r = await tun(m, p, { ...who, headers: forged, body: m === 'POST' ? {} : undefined });
      if (r.status !== want) wrong.push(`${m} ${p} ${r.status}`);
    }
    return wrong;
  };
  const asked = async (who = {}, forged = {}) => [...await denied(shut, 401, who, forged), ...await denied(hidden, 404, who, forged)];
  const strangerWrong = await asked();
  check('a stranger: the planner and the light page ask for a sign-in (401), what is for this machine only is not there (404)', strangerWrong.length === 0, strangerWrong.join('; '));
  for (const forged of [{ host: 'localhost' }, { 'x-forwarded-for': '127.0.0.1' }, { forwarded: 'for=127.0.0.1;host=localhost' }, { 'cf-connecting-ip': '127.0.0.1', 'x-real-ip': '127.0.0.1' }, { origin: base }]) {
    const wrong = await asked({}, forged);
    check(`a stranger who writes ${Object.entries(forged).map(([k, v]) => `${k}: ${v}`).join(', ')} is still a stranger`, wrong.length === 0, wrong.join('; '));
  }
  const fakeCookie = `__Host-hp_g=${'0'.repeat(64)}; herdr_planner_session=${'0'.repeat(64)}`;
  const wrongFake = await asked({ cookie: fakeCookie });
  check('a cookie nobody issued is no cookie', wrongFake.length === 0, wrongFake.join('; '));
  const wrongKey = await redeem('A'.repeat(22), IPHONE);
  check('a wrong key gets 410 and no cookie', wrongKey.status === 410 && wrongKey.cookies.length === 0, brief(wrongKey));
  const plain = await redeem(key, IPHONE, { headers: { 'cf-visitor': '{"scheme":"http"}' } });
  check('the right key over plain http is refused (403) and not spent', plain.status === 403 && plain.cookies.length === 0, brief(plain));

  // ---- a phone comes in with the key
  const phone = await redeem(key, IPHONE);
  const phoneCookie = cookieOf(phone, '__Host-hp_g');
  check('the phone redeems the link and waits for the owner', phone.status === 200 && phone.json?.state === 'pending' && /^__Host-hp_g=[0-9a-f]{64}$/.test(phoneCookie), brief(phone));
  const flags = phone.cookies[0]?.toLowerCase() || '';
  check('its cookie is a host cookie: HttpOnly, Secure, SameSite=Strict, Path=/, no Domain', ['httponly', 'secure', 'samesite=strict', 'path=/'].every((f) => flags.includes(f)) && !flags.includes('domain'), phone.cookies[0]);
  const again = await redeem(key, LINUX);
  check('the same key a second time (another device) is dead', again.status === 410 && again.cookies.length === 0, brief(again));
  const waitingWrong = await asked({ cookie: phoneCookie });
  check('a phone that waits for the owner is still a stranger', waitingWrong.length === 0, waitingWrong.join('; '));
  const v2 = await view();
  const waiting = v2.sessions.find((s) => s.state === 'pending');
  check('the owner sees it: the device named by what we know, the country, the light version', waiting?.label === 'iPhone · Safari 17' && waiting.country === 'RU' && waiting.scope === 'lite', JSON.stringify(waiting));
  check('the page of the owner never gets the spent key back', !JSON.stringify(v2).includes(key));
  const letIn = await own('POST', '/api/tunnel/session', { id: waiting.id, op: 'allow' });
  check('the owner lets it in', letIn.status === 200, brief(letIn));

  // ---- a phone that is in: the light page, and no more
  const lite = await tun('GET', '/m/api/state', { cookie: phoneCookie, ua: IPHONE });
  check('the light version: the snapshot for the phone', lite.status === 200 && Array.isArray(lite.json?.tasks) && lite.json.tasks.length > 0 && places(lite.json).length === 0, places(lite.json).slice(0, 3).join(', ') || brief(lite));
  const m = await tun('GET', '/m', { cookie: phoneCookie, ua: IPHONE, headers: page });
  check('the light page is served', m.status === 200 && /<script/.test(m.text), brief(m));
  const root = await tun('GET', '/', { cookie: phoneCookie, ua: IPHONE, headers: page });
  check('the planner\'s own page sends the phone to the light one', root.status === 303 && root.headers.location === '/m', brief(root));
  const planners = shut.filter(([, p]) => !p.startsWith('/m') && p !== '/assets/x.js' && p !== '/login');
  const liteWrong = await denied(planners, 403, { cookie: phoneCookie, ua: IPHONE });
  check('the light version is refused the planner\'s API (403)', liteWrong.length === 0, liteWrong.join('; '));
  const liteHidden = await denied(hidden, 404, { cookie: phoneCookie, ua: IPHONE });
  check('…and what is for this machine only is not there for it (404)', liteHidden.length === 0, liteHidden.join('; '));
  const liteBadAction = await tun('POST', '/m/api/action', { cookie: phoneCookie, ua: IPHONE, body: { action: 'anyway', id: 'alpha/2' } });
  const liteStop = await tun('POST', '/m/api/action', { cookie: phoneCookie, ua: IPHONE, body: { action: 'stop', id: 'alpha/2' } });
  check('the phone cannot «anyway» or stop an agent (403)', liteBadAction.status === 403 && liteStop.status === 403, `${liteBadAction.status} ${liteStop.status}`);
  const noPassword = await tun('POST', '/api/login', { cookie: phoneCookie, ua: IPHONE, body: { user: USER, password: PASSWORD } });
  check('with «the full planner» off the right password is not even looked at (403)', noPassword.status === 403 && !cookieOf(noPassword, 'herdr_planner_session'), brief(noPassword));
  const poll = await tun('GET', '/m/api/poll?seq=1', { cookie: phoneCookie, ua: IPHONE });
  check('the poll answers with a snapshot for a number that is not current', poll.status === 200 && poll.json?.seq > 1, brief(poll));

  // ---- the full version: only with the password
  const set = await own('POST', '/api/tunnel/settings', { full_allowed: true });
  check('the owner allows the full version for computers', set.status === 200 && (await view()).settings.full_allowed === true, brief(set));
  const rotated = await own('POST', '/api/tunnel/rotate', {});
  const key2 = keyOf(rotated.json);
  check('a new link brings a new key', rotated.status === 200 && key2 && key2 !== key, brief(rotated));
  const pc = await redeem(key2, LINUX);
  const pcCookie = cookieOf(pc, '__Host-hp_g');
  check('a computer redeems it and is sent to the planner\'s password', pc.status === 200 && pc.json?.next === '/login?next=/' && !!pcCookie, brief(pc));
  const pcWaiting = (await view()).sessions.find((s) => s.state === 'pending');
  await own('POST', '/api/tunnel/session', { id: pcWaiting.id, op: 'allow' });
  const pcState0 = await tun('GET', '/api/state', { cookie: pcCookie });
  check('before the password the computer has the light version only (403)', pcState0.status === 403, brief(pcState0));
  const wrongPw = await tun('POST', '/api/login', { cookie: pcCookie, body: { user: USER, password: 'nope' } });
  check('a wrong password raises nothing (401)', wrongPw.status === 401 && !cookieOf(wrongPw, 'herdr_planner_session'), brief(wrongPw));
  const rightPw = await tun('POST', '/api/login', { cookie: pcCookie, body: { user: USER, password: PASSWORD } });
  const pcFull = `${pcCookie}; ${cookieOf(rightPw, 'herdr_planner_session')}`;
  check('the right password raises it to the full version', rightPw.status === 200 && pcFull.includes('herdr_planner_session=') , brief(rightPw));
  const pcState = await tun('GET', '/api/state', { cookie: pcFull });
  check('the full version: the planner\'s state', pcState.status === 200 && Array.isArray(pcState.json?.tasks) && typeof pcState.json?.chat_rev === 'number', brief(pcState));
  const pcRoot = await tun('GET', '/', { cookie: pcFull, headers: page });
  check('the planner\'s page comes with the mark that tells it to ask instead of listening', pcRoot.status === 200 && /<meta name="herdr-gate" content="poll">/.test(pcRoot.text), brief(pcRoot));
  const pcEvents = await tun('GET', '/api/events', { cookie: pcFull });
  check('a quick tunnel has no event stream: 410 and the interval to ask at', pcEvents.status === 410 && pcEvents.json?.poll_ms === 5000, brief(pcEvents));
  const pcWrong = await denied(hidden, 404, { cookie: pcFull });
  check('the full version still has nothing of what is for this machine only (404): terminals, files, yards, station, memory, the tunnel', pcWrong.length === 0, pcWrong.join('; '));
  for (const forged of [{ host: 'localhost' }, { 'x-forwarded-for': '127.0.0.1' }, { forwarded: 'for=127.0.0.1' }, { origin: base, host: HOST }]) {
    const wrong = await denied(hidden, 404, { cookie: pcFull }, forged);
    check(`…whatever it writes: ${Object.entries(forged).map(([k, v]) => `${k}: ${v}`).join(', ')}`, wrong.length === 0, wrong.join('; '));
  }
  const yazi = await tun('POST', '/api/action', { cookie: pcFull, body: { action: 'yazi', id: 'alpha/2' } });
  check('opening files of a task (yazi) is for this machine: 403 from a tunnel', yazi.status === 403, brief(yazi));
  const other = await tun('GET', '/api/state', { cookie: `${phoneCookie}; ${cookieOf(rightPw, 'herdr_planner_session')}`, ua: IPHONE });
  check('the planner\'s cookie of one device does not raise another (403)', other.status === 403, brief(other));

  // ---- switching a device off
  const pcId = (await view()).sessions.find((s) => s.scope === 'full')?.id;
  const off = await own('POST', '/api/tunnel/session', { id: pcId, op: 'revoke' });
  check('the owner switches the computer off', off.status === 200 && !!pcId, brief(off));
  const after = [await tun('GET', '/api/state', { cookie: pcFull }), await tun('GET', '/m/api/state', { cookie: pcFull }), await tun('GET', '/api/state', { cookie: pcFull.split('; ')[1] })];
  check('its cookies, and the planner\'s cookie it earned, are of no use (401)', after.every((r) => r.status === 401), after.map((r) => r.status).join(' '));
  const stays = await tun('GET', '/m/api/state', { cookie: phoneCookie, ua: IPHONE });
  check('the other device stays in', stays.status === 200);
  const left = await tun('POST', '/m/api/logout', { cookie: phoneCookie, ua: IPHONE, body: {} });
  check('a device can leave on its own', left.status === 200 && (await tun('GET', '/m/api/state', { cookie: phoneCookie, ua: IPHONE })).status === 401, brief(left));
  const back = await own('POST', '/api/tunnel/rotate', {});
  const key3 = keyOf(back.json);
  const third = await redeem(key3, IPHONE);
  const thirdCookie = cookieOf(third, '__Host-hp_g');
  const thirdId = (await view()).sessions.find((s) => s.state === 'pending')?.id;
  await own('POST', '/api/tunnel/session', { id: thirdId, op: 'allow' });
  check('a device that is in can close nothing but the whole gate; the owner\'s calls are not its calls (404)', (await Promise.all(['open', 'rotate', 'session', 'settings', 'config', 'close'].map((p) => tun('POST', `/api/tunnel/${p}`, { cookie: thirdCookie, ua: IPHONE, body: {} })))).every((r) => r.status === 404));
  check('the gate is still open after those', (await view()).state === 'open');

  // ---- the brute-force stop: five wrong keys from one address
  let last = null;
  for (let i = 0; i < 6; i++) last = await tun('POST', '/g/redeem', { headers: { 'cf-connecting-ip': '198.51.100.7' }, body: { key: `${'B'.repeat(21)}${i}` } });
  check('five wrong keys from one address stop that address (429)', last.status === 429, brief(last));
  const blocked = await tun('POST', '/g/redeem', { headers: { 'cf-connecting-ip': '198.51.100.7' }, body: { key: key3 } });
  check('…even with a good key; another address is not affected', blocked.status === 429 && (await tun('GET', '/g/status', { cookie: thirdCookie, ua: IPHONE })).status === 200, brief(blocked));

  // ---- closing
  const closing = await own('POST', '/api/tunnel/close', {});
  check('the owner closes the gate', closing.status === 200, brief(closing));
  const gone = await waitView((v) => v.state === 'closed');
  check('closed: no link, no key, no devices', gone?.state === 'closed' && gone.link === null && gone.sessions.length === 0, JSON.stringify(gone?.state));
  const dead = [await tun('GET', '/m/api/state', { cookie: thirdCookie, ua: IPHONE }), await tun('POST', '/g/redeem', { body: { key: key3 } }), await tun('GET', '/g/status', { cookie: thirdCookie })];
  check('after closing everything through the tunnel is 503, the cookies and the keys of before included', dead.every((r) => r.status === 503), dead.map((r) => r.status).join(' '));
  const ownerStill = await own('GET', '/api/state');
  check('this machine is not affected', ownerStill.status === 200);
  const reopened = await own('POST', '/api/tunnel/open', {});
  const v4 = await waitView((v) => v.state === 'open');
  check('a gate opened again has a key of its own and knows none of the old cookies', reopened.status === 200 && keyOf(v4) && ![key, key2, key3].includes(keyOf(v4))
    && (await tun('GET', '/m/api/state', { cookie: thirdCookie, ua: IPHONE })).status === 401 && (await tun('GET', '/m/api/state', { cookie: pcFull })).status === 401);
} catch (e) {
  console.log(`FAIL the script broke: ${e?.stack || e}`);
  failed++;
} finally {
  // leave the gate closed and the setting as it was, whatever happened
  try { await own('POST', '/api/tunnel/close', {}); await own('POST', '/api/tunnel/settings', { full_allowed: false }); } catch { /* the server is gone */ }
}
console.log(failed ? `tunnel-check: ${failed} failed` : 'tunnel-check: ok');
process.exit(failed ? 1 : 0);
