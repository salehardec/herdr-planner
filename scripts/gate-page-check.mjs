#!/usr/bin/env node
// The sign-in page's script (src/gate_page/gate.js) against a stand-in browser: the key leaves the address bar before
// anything is sent, travels only in a POST body, and every answer of the server ends on the screen it should.
//   node scripts/gate-page-check.mjs        (cargo test runs it: devices::tests::the_sign_in_page_script_...)
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const source = readFileSync(join(root, 'src/gate_page/gate.js'), 'utf8');
const html = readFileSync(join(root, 'src/gate_page/gate.html'), 'utf8');
let failed = 0;
const check = (name, ok, extra = '') => {
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${name}${extra ? ' — ' + extra : ''}`);
  if (!ok) failed++;
};
const settle = async () => { for (let i = 0; i < 12; i++) await new Promise((r) => setImmediate(r)); };

// the page has no inline script (the planner's CSP forbids it) and loads the one script
check('the page has no inline script', !/<script(?![^>]*\bsrc=)/i.test(html) && /<script src="\/g\/gate\.js"><\/script>/.test(html));
check('the page links to nothing outside', !/https?:\/\//.test(html));

function browser({ hash = '', language = 'ru-RU', replies = [], touch = 0 } = {}) {
  const ids = [...html.matchAll(/\bid="(\w+)"/g)].map((m) => m[1]);
  const el = () => {
    const listeners = {};
    return { textContent: '', hidden: false, dataset: {}, value: '', placeholder: '', listeners, focused: false,
      setAttribute() {}, addEventListener(t, f) { listeners[t] = f; }, focus() { this.focused = true; } };
  };
  const els = Object.fromEntries(ids.map((i) => [i, el()]));
  const seen = { fetches: [], replaced: [], urls: [], nav: [] };
  const timers = [];
  const win = {
    document: { getElementById: (i) => els[i], documentElement: { lang: '' }, title: '' },
    navigator: { language, maxTouchPoints: touch },
    matchMedia: () => ({ matches: touch > 0 }),
    history: { replaceState: (_s, _t, url) => { seen.urls.push(url); win.location.hash = ''; seen.fetches.forEach(() => {}); seen.replacedBeforeFetch = seen.fetches.length === 0; } },
    location: { hash, pathname: '/', search: '', replace: (u) => seen.nav.push(u) },
    fetch: async (path, init = {}) => {
      seen.fetches.push({ path, method: init.method, body: init.body, hashThen: win.location.hash });
      const r = replies.shift();
      if (!r) throw new Error('no reply scripted for ' + path);
      if (r === 'network') throw new Error('offline');
      return { status: r.status, json: async () => r.body ?? {} };
    },
    setTimeout: (f) => { timers.push(f); return timers.length; },
    clearTimeout: (id) => { if (id) timers[id - 1] = null; },
    URLSearchParams, JSON, Object, String, Promise, RegExp, console,
  };
  win.window = win;
  vm.runInNewContext(source, win);
  return { els, seen, timers, win, tick: async () => { const f = timers.find(Boolean); if (f) { timers[timers.indexOf(f)] = null; f(); } await settle(); } };
}

const key = 'AbCdEfGhIjKlMnOpQrSt-_';

// 1. a link with a key: wiped first, sent once, pending, polled, in
{
  const b = browser({ hash: `#k=${key}`, touch: 5, replies: [
    { status: 200, body: { state: 'pending', computer: 'arch-box', label: 'iPhone · Safari 17', next: '/m' } },
    { status: 200, body: { state: 'pending' } },
    { status: 200, body: { state: 'active', next: '/m' } },
  ] });
  await settle();
  check('the key is wiped from the address bar before the request', b.seen.urls.length === 1 && b.seen.urls[0] === '/' && b.seen.replacedBeforeFetch === true, JSON.stringify(b.seen.urls));
  const first = b.seen.fetches[0];
  check('the key goes once, in the body of a POST, never in a URL', first.path === '/g/redeem' && first.method === 'POST' && JSON.parse(first.body).key === key && !b.seen.fetches.some((f) => f.path.includes(key)), JSON.stringify(first));
  check('a touch screen says so', JSON.parse(first.body).touch === true);
  check('the screen waits for the owner and names the machine', b.els.box.dataset.s === 'pending' && /arch-box/.test(b.els.p.textContent) && /iPhone · Safari 17/.test(b.els.p.textContent) && !b.els.cancel.hidden, `${b.els.h.textContent} / ${b.els.p.textContent}`);
  await b.tick();
  check('it asks what became of the request', b.seen.fetches[1].path === '/g/status' && b.seen.fetches[1].method === 'GET');
  check('still waiting', b.els.box.dataset.s === 'pending');
  await b.tick();
  check('let in: it goes on to the light page', b.seen.nav.length === 1 && b.seen.nav[0] === '/m' && b.els.box.dataset.s === 'arrived', JSON.stringify(b.seen.nav));
  check('the Russian words for a Russian browser', /Готово/.test(b.els.h.textContent));
}

// 2. the ends of a request
for (const [name, reply, state, word] of [
  ['a spent or wrong key', { status: 410, body: { error: 'x' } }, 'stale', /больше не работает/],
  ['too many attempts', { status: 429, body: { retry_s: 120 } }, 'slow', /120/],
  ['a full gate', { status: 409, body: {} }, 'full', /пять/],
  ['a closed gate', { status: 503, body: {} }, 'closed', /закрыт/],
  ['a server error', { status: 500, body: {} }, 'error', /Не получилось/],
  ['no network', 'network', 'error', /Не получилось/],
]) {
  const b = browser({ hash: `#k=${key}`, replies: [reply] });
  await settle();
  check(name, b.els.box.dataset.s === state && word.test(b.els.h.textContent + b.els.p.textContent), `${b.els.box.dataset.s}: ${b.els.h.textContent}`);
}
{
  const b = browser({ hash: `#k=${key}`, replies: [{ status: 429, body: { retry_s: 30 } }, { status: 410, body: {} }] });
  await settle();
  check('«try again» is offered after a refusal to wait', !b.els.again.hidden);
  b.els.again.listeners.click();
  await settle();
  check('…and asks again with the same key', b.seen.fetches.length === 2 && JSON.parse(b.seen.fetches[1].body).key === key && b.els.box.dataset.s === 'stale');
}

// 3. the other end of a request while waiting
for (const [name, reply, state] of [
  ['declined by the owner', { status: 200, body: { state: 'gone', reason: 'denied' } }, 'denied'],
  ['nobody answered', { status: 200, body: { state: 'gone', reason: 'unanswered' } }, 'denied'],
  ['switched off', { status: 200, body: { state: 'gone', reason: 'revoked' } }, 'gone'],
  ['the gate closed meanwhile', { status: 503, body: {} }, 'closed'],
]) {
  const b = browser({ hash: `#k=${key}`, replies: [{ status: 200, body: { state: 'pending', computer: 'c', label: 'l' } }, reply] });
  await settle();
  await b.tick();
  check(name, b.els.box.dataset.s === state, b.els.box.dataset.s);
}

// 4. no key in the address: the cookie says where we are, or a key can be pasted
{
  const b = browser({ hash: '', language: 'en-US', replies: [{ status: 200, body: { state: 'none' } }] });
  await settle();
  check('no key and no session: a field to paste a link', b.els.box.dataset.s === 'nokey' && !b.els.f.hidden && /link is needed/i.test(b.els.h.textContent), b.els.h.textContent);
  check('no history change when there was nothing to wipe', b.seen.urls.length === 0);
  b.els.k.value = `https://abc.trycloudflare.com/#k=${key}`;
  b.win.location.hash = '';
  b.els.f.listeners.submit({ preventDefault() {} });
  await settle();
  const post = b.seen.fetches.find((f) => f.path === '/g/redeem');
  check('a pasted link gives its key, and the field is emptied', !!post && JSON.parse(post.body).key === key && b.els.k.value === '', JSON.stringify(post));
}
{
  const b = browser({ hash: '', replies: [{ status: 200, body: { state: 'active', next: '/m' } }] });
  await settle();
  check('a browser that is in already goes on', b.seen.nav[0] === '/m');
}
{
  const b = browser({ hash: '', replies: [{ status: 200, body: { state: 'pending', computer: 'c', label: 'l' } }, { status: 200, body: {} }] });
  await settle();
  check('a reload while waiting goes on waiting', b.els.box.dataset.s === 'pending');
  b.els.cancel.listeners.click();
  await settle();
  check('«Cancel» withdraws the request', b.seen.fetches[1].path === '/g/cancel' && b.seen.fetches[1].method === 'POST' && b.els.box.dataset.s === 'nokey');
}
{
  // a next that is not a path of this site is not followed
  const b = browser({ hash: '', replies: [{ status: 200, body: { state: 'active', next: 'https://evil.example/' } }] });
  await settle();
  check('only a path of this site is followed', b.seen.nav[0] === '/m', JSON.stringify(b.seen.nav));
}
{
  const b = browser({ hash: '#k=short', replies: [{ status: 200, body: { state: 'none' } }] });
  await settle();
  check('a key of the wrong shape is not sent', !b.seen.fetches.some((f) => f.path === '/g/redeem') && b.els.box.dataset.s === 'nokey');
}

console.log(failed ? `\n${failed} FAILED` : '\nall ok');
process.exit(failed ? 1 : 0);
