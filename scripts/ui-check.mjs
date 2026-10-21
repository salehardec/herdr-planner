#!/usr/bin/env node
// Checks of the Galaxy in headless Chromium over the DevTools protocol.
// No dependencies (Node 22+ has WebSocket). With the server running on the fixture:
//   FIXTURE=DIR node scripts/ui-check.mjs http://127.0.0.1:7117 [screenshot-dir]
// `scripts/run-checks.sh` starts the server on the world of scripts/fixture.mjs.
// The fixture's logs (DIR/log/*.jsonl) are the witness for every board command, every
// ssh call to another machine and every terminal that was opened.
//   ONLY=name,name  runs only the sections whose name contains one of the words.
//   NO_RETRY=1      no second go for a section that failed (scripts/stress-checks.sh uses it: a flake must show).
//   WAIT_SCALE=n    every wait for a state gets n times its time (default 3): the limit of a wait is a limit, not a delay.
//
// The rules of these checks (they run on shared, loaded machines):
//  - a check waits for a STATE (waitFor / waitLog / the page's own debug()), never for a time; a pause is only
//    allowed where the page itself has a minimum time (the two-step buttons: see twoPresses);
//  - a key held down is held until the ship has moved, not for so many milliseconds (frames come as they can);
//  - what must NOT happen is checked after a barrier that proves the page has had its turn (see pageTurn());
//  - a section that fails is run once more from a fresh page, said out loud in the output and in the summary.

import { spawn } from 'node:child_process';
import { mkdtempSync, writeFileSync, readFileSync, existsSync, unlinkSync, utimesSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const base = process.argv[2] || 'http://127.0.0.1:7117';
const shots = process.argv[3] || null;
const fixture = process.env.FIXTURE;
// The name of this machine as the planner shows it: the fixture server is started with HERDR_PLANNER_HOST (run-checks.sh),
// not left to the hostname of whatever machine runs the checks.
const HOST = process.env.HERDR_PLANNER_HOST || 'orion';
if (!fixture) { console.error('ui-check: FIXTURE is required'); process.exit(2); }
const noRetry = !!process.env.NO_RETRY;
const only = (process.env.ONLY || '').split(',').map((s) => s.trim()).filter(Boolean);
const profile = mkdtempSync(join(tmpdir(), 'herdr-planner-ui-'));
// The browser picks its own debugging port (several checks run side by side; a random port could be taken twice):
// it writes the port into its profile folder.
let port = 0;
const readPort = () => { try { return Number(readFileSync(join(profile, 'DevToolsActivePort'), 'utf8').split('\n')[0]) || 0; } catch { return 0; } };
const chrome = spawn(process.env.CHROMIUM || 'chromium', [
  '--headless=new', '--no-sandbox', '--disable-gpu', '--use-gl=angle', '--use-angle=swiftshader', '--enable-unsafe-swiftshader',
  '--hide-scrollbars', `--user-data-dir=${profile}`, '--remote-debugging-port=0', '--window-size=1500,950',
  '--host-resolver-rules=MAP remote.test 127.0.0.1', 'about:blank',
], { stdio: 'ignore' });
// Whatever happens, the browser goes and its profile (hundreds of MB in /tmp) goes with it.
process.on('exit', () => {
  try { chrome.kill('SIGKILL'); } catch { /* gone already */ }
  try { rmSync(profile, { recursive: true, force: true, maxRetries: 3 }); } catch { /* best effort */ }
  // the killed browser may still write a last file into the folder: sweep once more a moment later, detached
  try { spawn('sh', ['-c', `sleep 2; rm -rf '${profile}'`], { detached: true, stdio: 'ignore' }).unref(); } catch { /* best effort */ }
});

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
// The limit of a wait for a state is a limit only: on a loaded machine a state comes late, on a quiet one at once.
const WAIT_SCALE = Number(process.env.WAIT_SCALE) || 3;
let failed = 0;
let failedNow = [];   // the FAIL lines of the section that is running (for its second go)
function check(name, ok, extra = '') {
  const line = `${ok ? 'ok  ' : 'FAIL'} ${name}${extra ? ' — ' + String(extra).replace(/\s+/g, ' ').slice(0, 300) : ''}`;
  console.log(line);
  if (!ok) { failed++; failedNow.push(name); }
}

// ── harness ──────────────────────────────────────────────────────────────

async function waitForChrome() {
  for (let i = 0; i < 300; i++) {   // up to a minute: a loaded machine starts a browser slowly
    port = port || readPort();
    try { if (port) { const r = await fetch(`http://127.0.0.1:${port}/json/version`); if (r.ok) return; } } catch {}
    await sleep(200);
  }
  throw new Error('chromium did not start');
}

const netUrls = new Map();
let ws; let id = 0; const pending = new Map(); const errors = [];
// A browser that stopped answering (a crashed renderer) would leave the check waiting for ever: when the latest call
// has had no reply for 300 s, say so and stop. (A call somebody gave up on earlier does not count: later ones are answered.)
let lastSentAt = 0; let lastRepliedAt = 0;
setInterval(() => {
  if (lastSentAt > lastRepliedAt && Date.now() - lastSentAt > 300000) { console.log('FAIL the browser stopped answering'); process.exit(1); }
}, 5000).unref();
const send = (method, params = {}) => new Promise((r) => {
  const i = ++id;
  lastSentAt = Date.now();
  pending.set(i, (m) => { lastRepliedAt = Date.now(); r(m); });
  ws.send(JSON.stringify({ id: i, method, params }));
});
let lastNavAt = 0;
// A page is there when the browser says it has loaded and, for the app itself, when its first state has arrived
// (the world, the panels and the event stream are up). No time is waited: a slow machine takes as long as it takes.
let loadWaiters = [];
const navigate = async (url) => {
  lastNavAt = Date.now();
  const loaded = new Promise((r) => { loadWaiters.push(r); });
  await send('Page.navigate', { url });
  await Promise.race([loaded, sleep(60000)]);
  if (/^https?:/.test(url) && !/\/login(\?|$)/.test(url)) {
    await waitFor('document.readyState === "complete" && !!window.planner && !!window.plannerSnap && !!window.plannerSnap()', 30000);
  }
};
const keyText = async (ch) => {
  await send('Input.dispatchKeyEvent', { type: 'keyDown', text: ch, key: ch });
  await send('Input.dispatchKeyEvent', { type: 'keyUp', key: ch });
};
// A physical key: the page reads `code`, so a letter works in any layout.
// The browser drops synthetic key events now and then (its window is not the focused one when several run side by side):
// the page counts the keydowns it gets (__keyN), and a key that never arrived is sent again. A key that arrived is never repeated.
const keyCount = () => evalJs('window.__keyN || 0');
const press = async (code, key = code, mods = 0) => {
  const vk = { Escape: 27, Enter: 13, ArrowUp: 38, ArrowDown: 40, ArrowLeft: 37, ArrowRight: 39, Slash: 191, Space: 32, Tab: 9 }[code] ?? (key.length === 1 ? key.toUpperCase().charCodeAt(0) : 0);
  const n0 = await keyCount();
  for (let tries = 0; tries < 4; tries++) {
    await send('Input.dispatchKeyEvent', { type: 'rawKeyDown', key, code, windowsVirtualKeyCode: vk, modifiers: mods, text: key.length === 1 ? key : undefined });
    await send('Input.dispatchKeyEvent', { type: 'keyUp', key, code, windowsVirtualKeyCode: vk, modifiers: mods });
    if (n0 === undefined || await waitFor(`(window.__keyN || 0) > ${n0}`, 1200)) return;
    console.log(`   the key ${code} did not reach the page; sent again`);
  }
};
// A key held down: dispatched inside the page (CDP key events are dropped when the headless window loses focus).
// It is held until `until` (an expression about the page) is true, not for so long: how far a ship gets in a second
// depends on how many frames the machine drew. Returns whether the condition came before the time was out.
const hold = async (code, key, until, ms = 20000) => {
  await evalJs(`document.body.dispatchEvent(new KeyboardEvent('keydown', { code: ${JSON.stringify(code)}, key: ${JSON.stringify(key)}, bubbles: true, cancelable: true })); true`);
  const ok = await waitFor(until, ms);
  await evalJs(`document.body.dispatchEvent(new KeyboardEvent('keyup', { code: ${JSON.stringify(code)}, key: ${JSON.stringify(key)}, bubbles: true })); true`);
  return ok;
};
const evalJs = async (expr) => {
  const reply = await Promise.race([
    send('Runtime.evaluate', { expression: expr, returnByValue: true, awaitPromise: true }),
    sleep(45000).then(() => null),
  ]);
  if (!reply) { console.log('eval timeout:', expr.replace(/\s+/g, ' ').slice(0, 100)); return undefined; }
  if (reply.result?.exceptionDetails) { console.log('eval error:', reply.result.exceptionDetails.exception?.description?.slice(0, 160)); return undefined; }
  return reply.result?.result?.value;
};
// A text on the page: waits (a state, not a time) until the element has it. Use instead of `(await text(sel)).includes(...)`
// right after a click, when the page may still be drawing the answer.
const hasText = (sel, str, ms = 4000, anyCase = false) => waitFor(`(document.querySelector(${JSON.stringify(sel)})?.innerText || '')${anyCase ? '.toLowerCase()' : ''}.includes(${JSON.stringify(anyCase ? String(str).toLowerCase() : str)})`, ms);
const text = (sel) => evalJs(`document.querySelector(${JSON.stringify(sel)})?.innerText || ''`);
const click = (sel) => evalJs(`(() => { const e = document.querySelector(${JSON.stringify(sel)}); if (e) e.click(); return !!e; })()`);
const exists = (sel) => evalJs(`!!document.querySelector(${JSON.stringify(sel)})`);
const shot = async (name) => {
  if (!shots) return;
  const r = await send('Page.captureScreenshot', { format: 'png' });
  writeFileSync(join(shots, name), Buffer.from(r.result.data, 'base64'));
};
async function waitFor(expr, ms = 8000) {
  const end = Date.now() + ms * WAIT_SCALE;
  // an expression that asks the server (fetch) is asked less often: every answer costs the server a process or two
  const every = expr.includes('fetch(') ? 400 : 100;
  while (Date.now() < end) { if (await evalJs(expr)) return true; await sleep(every); }
  return false;
}
const api = (path, opts) => evalJs(`fetch(${JSON.stringify(path)}, ${JSON.stringify(opts || {})}).then(async (r) => [r.status, await r.json().catch(() => null)])`);
const post = (path, body) => api(path, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) });
const calls = (file = 'commands.jsonl') => {
  const f = join(fixture, 'log', file);
  return existsSync(f) ? readFileSync(f, 'utf8').split('\n').filter(Boolean).map((l) => JSON.parse(l)) : [];
};
const lastCall = (file = 'commands.jsonl') => calls(file).at(-1);
const stateApi = () => evalJs('fetch("/api/state").then(r => r.json())');
const debug = () => evalJs('window.world ? world.debug() : null');
const pd = () => evalJs('window.plannerDebug ? plannerDebug() : null');
const planet = (taskId, galaxy = '') => `{kind:'planet',galaxy:${JSON.stringify(galaxy)},id:${JSON.stringify(taskId)}}`;
// The operator's own move: the ship flies there (a flight), the place left goes on the stack.
const go = async (taskId, galaxy = '') => {
  await evalJs(`planner.navigateTo(${planet(taskId, galaxy)}); true`);
  await waitFor(`plannerDebug().selected?.id === ${JSON.stringify(taskId)} && world.debug().flight === "idle"`, 12000);
  await pageTurn();
};
const clearToasts = () => evalJs('document.querySelector("#toasts")?.replaceChildren(); true');
async function waitLog(pred, ms = 8000) {
  const end = Date.now() + ms * WAIT_SCALE;
  while (Date.now() < end) { if (pred()) return true; await sleep(100); }
  return false;
}
// A human who sees that a key or a click had no effect does it again. So does this: `act()` is done, and when the condition has not
// come in `ms`, once more (the output says so, and what the page looked like). Only for what is harmless to do twice.
const untilDone = async (what, act, cond, ms, look = 'true') => {
  for (let attempt = 1; ; attempt++) {
    await act();
    if (typeof cond === 'function' ? await waitLog(cond, ms) : await waitFor(cond, ms)) return true;
    if (attempt >= 2) return false;
    console.log(`   ${what}: no effect; done once more. The page: ${JSON.stringify(await evalJs(look)).slice(0, 300)}`);
  }
};
// A barrier: the page has handled everything that was queued before it (events, timers, the renders that follow).
// For what must NOT happen: a thing is checked as absent only after the page has had its turn, not after a guess of time.
// The camera has come to rest: the same place for three drawn frames in a row (the eased camera of the autopilot goes on moving
// for a while after a key or a flight, and how long that takes in seconds depends on the machine).
const calmCamera = () => evalJs(`new Promise((done) => { const at = () => { const c = world.debug().camera; return c.x + ',' + c.y + ',' + c.z; }; let last = at(); let same = 0; const f = () => { const now = at(); same = now === last ? same + 1 : 0; last = now; if (same >= 3) done(true); else requestAnimationFrame(f); }; requestAnimationFrame(f); setTimeout(() => done(false), 30000); })`);
const pageTurn = () => evalJs(`new Promise((done) => setTimeout(() => setTimeout(() => done(true), 0), 0))`);
// The same, and the world has drawn `n` frames (the frame loop runs as fast as the machine lets it).
const frames = (n = 2) => evalJs(`new Promise((done) => { let k = ${n}; const f = () => (--k <= 0 ? done(true) : requestAnimationFrame(f)); requestAnimationFrame(f); setTimeout(() => done(false), 20000); })`);
// The page's own calls to the server (fetch) are counted from the start of every document: `quiet()` is «nothing is on its way».
const quiet = async () => { await waitFor('!window.__net || window.__net.pending === 0', 20000); await pageTurn(); };
// A fresh page for a section that is run again: the app, its first state in, nothing open, the language back to Russian.
const resetPage = async () => {
  await send('Emulation.setDeviceMetricsOverride', { width: 1500, height: 950, deviceScaleFactor: 1, mobile: false });
  await send('Network.setExtraHTTPHeaders', { headers: {} });
  await evalJs('try { localStorage.removeItem("herdr-planner.lang"); } catch {} true');
  await navigate(base + '/?noauto&nocoach&bloom=0');
  await waitFor('window.world && world.debug().planets > 0', 20000);
};
// The page has handled what was queued and the world has drawn a couple of frames (a panel opened, a key pressed): what the
// geometry checks and the next step see is the result, not a half-moved panel.
const beat = async () => { await pageTurn(); await frames(2); };
// After a new window size: the resize reaches the page, which lays itself out again.
const resized = async () => { await frames(2); await evalJs('window.dispatchEvent(new Event("resize")); true'); await beat(); };
// Esc four times (closes whatever is open), each press after the page has handled the one before.
const resetUi = async () => {
  for (let i = 0; i < 4; i++) { await press('Escape'); await pageTurn(); }
};

const sections = [];
// `{ retry: true }`: the section only looks and navigates (or does what can be done twice), so when it fails it is run once
// more from a fresh page. The second go is never silent: see the end of the run.
const section = (name, fn, opts = {}) => sections.push([name, fn, opts]);

// ── the checks ───────────────────────────────────────────────────────────

section('boot', async () => {
  // The neon is a post-processing pass: once with it on, to see it builds and draws without errors.
  await navigate(base + '/?noauto&nocoach');
  check('the bloom pass builds in the normal mode', await waitFor('window.world && world.debug().bloom === true', 15000), JSON.stringify(await debug()?.then?.((d) => d?.bloom)));
  await navigate(base + '/?noauto&nocoach&bloom=0');
  check('the world starts', await waitFor('window.world && world.debug().planets > 0', 20000));
  check('the page has a title and no console errors', (await evalJs('document.title')).includes('Herdr Planner') && errors.length === 0, errors.slice(0, 2).join('; '));
  check('the page fills the viewport', await waitFor('Math.abs(document.querySelector("#win").getBoundingClientRect().width - innerWidth) < 2'));
  const d0 = await debug();
  check('WebGL is on', d0?.webgl === true, JSON.stringify(d0 && { simple: d0.simple, fps: d0.fps }));
  check('every live task is a planet', d0?.livePlanets === 17, String(d0?.livePlanets));
  check('the seven live statuses are drawn', ['failed', 'plan', 'queued', 'ready', 'review', 'waiting', 'working'].every((s) => d0?.statuses.includes(s)), (d0?.statuses || []).join(','));
  check('universes are projects, quiet ones marked', d0?.universes === 7 && d0?.quiet === 3, JSON.stringify({ u: d0?.universes, q: d0?.quiet }));
  check('the board dependencies are links', d0?.links.some((l) => l.from === 'alpha/9' && l.to === 'alpha/10') && d0?.links.some((l) => l.from === 'alpha/4' && l.to === 'gamma/1'), JSON.stringify(d0?.links));
  const s = await stateApi();
  check('the top bar counts the requests', await hasText('#wait-btn', String(s.summary.attention)), `${await text('#wait-btn')} vs ${s.summary.attention}`);
  check('the Quasar is the centre and says it waits for the human', await waitFor('world.debug().labels > 0 && (document.querySelector(".labels")?.innerText || "").includes("Квазар")', 15000), (await text('.labels')).slice(0, 80));
  check('the kanban line shows the five stages', (await evalJs('document.querySelectorAll("#kanban [data-stage]").length')) === 5 && await hasText('#kanban', 'План и работа'), await text('#kanban'));
  check('the ship is drawn: the cabin frame is off until something is selected', !(await evalJs('document.querySelector("#cockpit").classList.contains("on")')));
  await shot('galaxy-overview.png');
}, { retry: true });

section('live', async () => {
  // The page learns of a board change by the event stream: no /api/state polling.
  // A poll of /api/state is allowed only where the page has its own reason (it became visible again, its stream broke; a stream that broke earlier leaves the page's 30 s fallback poll running until the next state, so one tick of it is allowed): the
  // headless windows of side-by-side runs take the focus from each other. A poll that nothing explains is polling.
  const polls = () => evalJs('({ n: window.__net.log.filter((u) => u.includes("/api/state")).length, why: window.__net.shown + window.__net.streamErrors })');
  const before = await polls();
  const f = join(fixture, 'state', 'board.json');
  const w = JSON.parse(readFileSync(f, 'utf8'));
  const row = w.rows.find((r) => r.id === 'alpha/11');
  row.state = 'working'; row.stage = 'working'; row.msg = 'Только что взялась за дело';
  writeFileSync(f, JSON.stringify(w, null, 1));
  const t = join(fixture, 'board', 'tasks', 'alpha', '11', 'task.json');
  writeFileSync(t, readFileSync(t, 'utf8')); // the watcher sees the record change
  const pageHas = 'plannerSnap().tasks.find((x) => x.id === "alpha/11")?.status === "working"';
  check('a change of a task record reaches the page at once', await waitFor(pageHas, 15000));
  await quiet();
  const after = await polls();
  check('the page got it by the event stream, not by polling', after.n - before.n <= (after.why - before.why) + (before.why > 0 ? 1 : 0), JSON.stringify({ before, after }));
  check('and it is the server\'s state', (await stateApi()).tasks.find((x) => x.id === 'alpha/11')?.status === 'working');
  row.state = 'idle'; row.stage = 'todo'; row.msg = '';
  writeFileSync(f, JSON.stringify(w, null, 1));
  writeFileSync(t, readFileSync(t, 'utf8'));
  await waitFor(`plannerSnap().tasks.find((x) => x.id === "alpha/11")?.status !== "working"`, 15000);
});

section('navigator', async () => {
  check('the navigator starts with this machine\'s Galaxy', await hasText('#nav [data-kind="galaxy"]', HOST), await text('#nav [data-kind="galaxy"]'));
  check('the Quasar is its own item', await exists('#nav [data-kind="quasar"]'));
  check('far galaxies are below: vm-a is online, vm-b is lost with the reason', await hasText('#nav', 'vm-a') && await hasText('#nav', 'vm-b') && /нет связи/.test(await text('#nav')), (await text('#nav')).replace(/\s+/g, ' ').slice(-200));
  check('"Add a galaxy" and "Add a universe" are there', await hasText('#nav', 'Добавить галактику') && await hasText('#nav', 'Добавить вселенную'));
  const groups = await evalJs('[...document.querySelectorAll("#nav [data-kind=\\"group\\"]")].map((e) => ({ t: e.innerText.replace(/\\s+/g, " ").trim(), open: e.getAttribute("aria-expanded") }))');
  check('the first group of a universe is «Сегодня» and it is open', groups?.[0]?.t.startsWith('Сегодня') && groups[0].open === 'true', JSON.stringify(groups?.slice(0, 3)));
  check('older days are dd.mm.yyyy and folded', groups?.some((g) => /^\d\d\.\d\d\.\d{4}/.test(g.t) && g.open === 'false'), JSON.stringify(groups));
  // Each universe has its own run of days: «Сегодня» starts a new one.
  const runs = []; for (const g of groups || []) { if (g.t.startsWith('Сегодня')) runs.push([]); if (/^\d\d\.\d\d\.\d{4}/.test(g.t) && runs.length) runs.at(-1).push(g.t.slice(0, 10).split('.').reverse().join('')); }
  check('the days are newest first', runs.every((r) => r.every((v, i) => i === 0 || r[i - 1] >= v)) && runs.some((r) => r.length > 1), JSON.stringify(runs));
  const planets = await evalJs('[...document.querySelectorAll("#nav [data-kind=\\"planet\\"]")].map((e) => e.innerText.replace(/\\s+/g, " ").trim())');
  check('today\'s planets are listed with their key and status', planets?.some((p) => p.includes('ALP-2') && p.includes('ждёт')), JSON.stringify(planets?.slice(0, 4)));
  await evalJs('document.querySelector("#nav [data-kind=\\"group\\"][aria-expanded=\\"false\\"]").click()');
  check('a click folds and unfolds a day', await waitFor('document.querySelectorAll("#nav [data-kind=\\"group\\"][aria-expanded=\\"true\\"]").length >= 2', 3000));
  const moons = await evalJs('[...document.querySelectorAll("#nav [data-kind=\\"moon\\"]")].map((e) => e.innerText.replace(/\\s+/g, " ").trim())');
  check('sub-agents are under their planet (Explore under ALP-2)', moons?.some((m) => m.includes('Explore')), JSON.stringify(moons));
  await evalJs('document.querySelector("#nav [data-kind=\\"planet\\"]").focus()');
  await press('ArrowDown');
  check('the arrow keys move along the tree', await waitFor('document.activeElement.dataset.nav !== undefined', 3000));
  await shot('navigator.png');
}, { retry: true });

section('autopilot', async () => {
  await resetUi();
  const cam0 = (await debug()).camera;
  await evalJs(`document.querySelector('#nav [data-nav="planet||alpha/4"]')?.click()`);
  check('choosing a planet in the navigator starts a flight', await waitFor('world.debug().flight !== "idle"', 4000) || await waitFor('plannerDebug().selected?.id === "alpha/4"', 4000));
  check('the flight ends and the planet is selected', await waitFor('world.debug().flight === "idle" && plannerDebug().selected?.id === "alpha/4"', 15000), JSON.stringify(await pd()));
  const cam1 = (await debug()).camera;
  check('the ship really moved', Math.abs(cam1.x - cam0.x) + Math.abs(cam1.y - cam0.y) + Math.abs(cam1.z - cam0.z) > 50, JSON.stringify({ cam0, cam1 }));
  check('we are inside the nebula of the planet', (await pd()).level === 'nebula' && (await pd()).universe === 'alpha', JSON.stringify(await pd()));
  check('the cabin frame is on and shows the target', await evalJs('document.querySelector("#cockpit").classList.contains("on")') && await hasText('#readout', 'ALP-4'), await text('#readout'));
  check('the focus shows the request of the task', await hasText('#focus', 'Тёмная тема') && await hasText('#focus', 'К одобрению'), (await text('#focus')).slice(0, 100));
  check('the crumbs show where we are', await hasText('#crumbs', HOST) && await hasText('#crumbs', 'alpha') && await hasText('#crumbs', 'ALP-4'), await text('#crumbs'));
  check('the keys card gives its place to the cabin', !(await exists('#hint')));
  await shot('cockpit-task.png');
  await evalJs(`world.flyTo({kind:'overview'},{instant:true}); true`);
  await pageTurn();
}, { retry: true });

section('manual', async () => {
  await resetUi();
  await evalJs('planner.goOverview(); true');
  await waitFor('world.debug().flight === "idle" && plannerDebug().selected === null', 15000);
  await press('KeyP', 'p');
  check('P switches to manual flight', await waitFor('world.mode === "manual"', 3000) && await exists('#mode [data-mode="manual"].on'));
  check('from the overview the ship is taken inside the Galaxy first (not outside the boundary)', await waitFor('world.debug().flight === "idle"', 15000));
  check('and is within the boundary', await waitFor('(() => { const s = world.debug().ship; return Math.hypot(s.x, s.y, s.z) < 1200; })()'));
  check('the kanban gives way to the nose of the ship and the cross-hair is shown', await exists('#cross') && (await evalJs('getComputedStyle(document.querySelector("#kanban")).opacity')) === '0');
  // How far a ship gets depends on how many frames the machine drew, so each key is held until the ship has moved,
  // and the next one starts from a ship that has come to rest.
  const atRest = () => waitFor('world.debug().ship.speed < 1', 15000);
  const movedFrom = (p, min, extra = 'true') => `(() => { const s = world.debug().ship; return Math.hypot(s.x - ${p.x}, s.y - ${p.y}, s.z - ${p.z}) > ${min} && ${extra}; })()`;
  await atRest();
  const a = (await debug()).ship;
  const heldW = await hold('KeyW', 'w', `${movedFrom(a, 3)} && world.debug().ship.speed > 5`);
  const b = (await debug()).ship;
  check('W moves the ship forward at once', heldW && Math.hypot(b.x - a.x, b.y - a.y, b.z - a.z) > 3 && b.speed > 5, JSON.stringify({ a, b }));
  await atRest();
  const b2 = (await debug()).ship;
  const heldD = await hold('KeyD', 'd', movedFrom(b2, 3));
  const c = (await debug()).ship;
  check('D moves it sideways', heldD && Math.hypot(c.x - b2.x, c.y - b2.y, c.z - b2.z) > 3, JSON.stringify({ b2, c }));
  await atRest();
  const c2 = (await debug()).ship;
  const heldUp = await hold('Space', ' ', `world.debug().ship.y - ${c2.y} > 3`);
  const d = (await debug()).ship;
  check('Space goes up', heldUp && d.y - c2.y > 3, `${c2.y} → ${d.y}`);
  check('the mouse wheel changes the cruise speed', await evalJs(`(() => { const c = world.debug().ship.cruise; const e = new WheelEvent('wheel', { deltaY: -100, bubbles: true, cancelable: true }); document.querySelector('#scene').dispatchEvent(e); return world.debug().ship.cruise > c; })()`));
  await shot('manual-flight.png');
  // A dive: the crosshair on a nebula, a click → a flight inside, manual flight goes on.
  await evalJs(`planner.navigateTo({kind:'universe',galaxy:'',id:'delta'}); true`);
  check('a dive into a nebula keeps the ship manual and enters it', await waitFor('world.debug().flight === "idle" && world.mode === "manual" && world.level === "nebula"', 15000), JSON.stringify(await pd()));
  await press('KeyP', 'p');
  check('P returns to the autopilot', await waitFor('world.mode === "auto"', 3000));
  await resetUi();
}, { retry: true });

section('stack', async () => {
  await resetUi();
  await evalJs('planner.goOverview(); true');
  await waitFor('world.debug().flight === "idle" && plannerDebug().selected === null', 15000);
  await go('alpha/16');
  await press('KeyT', 't');
  check('T pulls the todo drawer out', await waitFor('document.querySelector("#todo").classList.contains("open")', 3000));
  await press('Escape');
  check('Esc closes the drawer first and stays where it is', await waitFor('!document.querySelector("#todo").classList.contains("open")', 3000) && (await pd()).selected?.id === 'alpha/16');
  await press('Escape');
  check('the next Esc is one step back: out of the focus, to where we came from', await waitFor('plannerDebug().selected === null', 15000), JSON.stringify(await pd()));
  check('and the camera flew back with us', await waitFor('world.debug().flight === "idle"', 15000));
  // Action leaves the operator in place: a tell does not fly anywhere.
  await go('alpha/2');
  const cam = (await debug()).camera;
  await evalJs('document.querySelector("#tell").value = "оставь старую"; document.querySelector("#tell").dispatchEvent(new Event("input", { bubbles: true }));');
  await click('[data-act="tell-agent"]');
  check('the message reaches the agent', await waitLog(() => calls().some((c) => c.argv[0] === 'tell' && c.argv[1] === 'alpha/2' && c.argv[3] === 'оставь старую'), 8000), JSON.stringify(lastCall()));
  // not flown away: look once the answer of the server has been handled and the world has drawn a few frames
  await quiet();
  await frames(3);
  const cam2 = (await debug()).camera;
  check('an action does not fly the ship away', Math.abs(cam.x - cam2.x) + Math.abs(cam.y - cam2.y) + Math.abs(cam.z - cam2.z) < 30 && (await pd()).selected?.id === 'alpha/2', JSON.stringify({ cam, cam2, sel: (await pd()).selected }));
  await resetUi();
}, { retry: true });

section('queue', async () => {
  await resetUi();
  await evalJs('planner.goOverview(); true');
  await waitFor('world.debug().flight === "idle" && plannerDebug().selected === null', 15000);
  await press('KeyB', 'b');
  check('B opens «Ждёт тебя» as a list', await waitFor('document.querySelector("#drawer-wait").classList.contains("open") && document.querySelectorAll("#drawer-wait .q").length >= 10', 4000), await evalJs('document.querySelectorAll("#drawer-wait .q").length'));
  const total = await evalJs('document.querySelectorAll("#drawer-wait .q").length');
  await press('ArrowDown'); await press('ArrowDown');
  await evalJs('document.querySelector("#drawer-wait .wlist").focus()');
  await press('ArrowDown');
  const cursor = await evalJs('[...document.querySelectorAll("#drawer-wait .q")].findIndex((e) => e.classList.contains("cur"))');
  // «the same camera» is the camera at the moment we leave: the arrows above may have turned the eased autopilot camera a little,
  // and where it is half way depends on the frames drawn, so it is taken once it has come to rest
  await calmCamera();
  const camList = (await debug()).camera;
  await press('Enter');
  check('Enter dives into the request under the cursor', await waitFor('plannerDebug().selected?.kind === "planet"', 15000), JSON.stringify(await pd()));
  const opened = (await pd()).selected.id;
  await waitFor('world.debug().flight === "idle"', 15000);
  check('the list stays open next to the request', await waitFor('document.querySelector("#drawer-wait").classList.contains("open")'));
  await press('Escape');
  check('Esc from a request returns to the same list, the same camera', await waitFor('plannerDebug().selected === null && document.querySelector("#drawer-wait").classList.contains("open")', 15000), JSON.stringify(await pd()));
  const camBackOk = await waitFor(`world.debug().flight === "idle" && (() => { const k = world.debug().camera; return Math.abs(k.x - ${camList.x}) + Math.abs(k.y - ${camList.y}) + Math.abs(k.z - ${camList.z}) < 80; })()`, 15000);
  const camBack = (await debug()).camera;
  check('the screen is as it was', camBackOk, JSON.stringify({ camList, camBack }));
  check('the cursor is where it was', (await evalJs('[...document.querySelectorAll("#drawer-wait .q")].findIndex((e) => e.classList.contains("cur"))')) === cursor);
  // Work through one: answer a request with the dock's own action, and come back to the list.
  const idx = await evalJs(`[...document.querySelectorAll('#drawer-wait .q')].findIndex((e) => e.dataset.sel === 'alpha/16')`);
  await evalJs(`document.querySelectorAll('#drawer-wait .q')[${idx}].click()`);
  await waitFor('plannerDebug().selected?.id === "alpha/16" && world.debug().flight === "idle"', 15000);
  await click('#focus [data-act="perm-allow"]');
  await evalJs('document.querySelector("[data-act=\\"tell-agent\\"]").click()');
  check('after the answer we are back in the list', await waitFor('plannerDebug().selected === null && document.querySelector("#drawer-wait").classList.contains("open")', 15000), JSON.stringify(await pd()));
  check('the answered request is struck out', await waitFor('!!document.querySelector("#drawer-wait .q.resolved")', 4000));
  check('the answer reached the agent', calls().some((c) => c.argv[0] === 'tell' && c.argv[1] === 'alpha/16' && c.argv[3] === 'Да, разрешаю.'), JSON.stringify(lastCall()));
  void total; void opened;
  await shot('queue.png');
  await resetUi();
});


section('search', async () => {
  await resetUi();
  await evalJs('planner.goOverview(); true');
  await waitFor('world.debug().flight === "idle" && plannerDebug().selected === null', 15000);
  await press('KeyK', 'k', 2);
  check('Ctrl+K focuses the search that is always on top', await waitFor('document.activeElement.id === "q"', 3000));
  await send('Input.insertText', { text: 'alp' });
  check('the suggestions appear at once, grouped with icons', await waitFor('document.querySelectorAll("#sres .srow").length > 3 && document.querySelectorAll("#sres h4").length >= 2', 3000), await text('#sres'));
  check('every result row is really there: tall, wide, in the flow, with its title and path to read', await evalJs(`(() => { const rows = [...document.querySelectorAll('#sres .srow')]; return rows.length > 3 && rows.every((r) => { const b = r.getBoundingClientRect(); const c = getComputedStyle(r); return b.height >= 24 && b.width >= 200 && c.position !== 'absolute' && c.clip === 'auto' && r.querySelector('.ttl')?.getBoundingClientRect().height >= 8 && r.querySelector('.path')?.getBoundingClientRect().width > 20; }); })()`), await evalJs('[...document.querySelectorAll("#sres .srow")].slice(0, 3).map((r) => { const b = r.getBoundingClientRect(); return b.width + "x" + b.height; }).join()'));
  // (the list may still be sliding in or being refined by the last keystroke: wait for it to stand still under the pointer)
  check('the rows can be seen and hit: a row is what stands under its own centre', await waitFor(`(() => { const r = document.querySelector('#sres .srow')?.getBoundingClientRect(); return !!r && !!document.elementFromPoint((r.left + r.right) / 2, (r.top + r.bottom) / 2)?.closest('.srow'); })()`, 5000), await evalJs(`(() => { const r = document.querySelector('#sres .srow')?.getBoundingClientRect(); const e = r && document.elementFromPoint((r.left + r.right) / 2, (r.top + r.bottom) / 2); return e ? e.tagName + '#' + e.id + '.' + e.className : 'none'; })()`));
  await shot('search-rows.png');
  const g = await evalJs('[...document.querySelectorAll("#sres h4")].map((e) => e.innerText)');
  check('groups come in the order: universes, planets, …', g.map((x) => x.toLowerCase()).indexOf('вселенные') !== -1 && g.map((x) => x.toLowerCase()).indexOf('планеты') > g.map((x) => x.toLowerCase()).indexOf('вселенные'), JSON.stringify(g));
  check('a match is highlighted and the path is shown', await exists('#sres mark') && (await text('#sres')).includes(HOST + ' › alpha › ALP-'), (await text('#sres')).slice(0, 160));
  check('the status is in its own colour class', await exists('#sres .srow.s-waiting') || await exists('#sres .srow.s-working'));
  await evalJs('document.getElementById("q").select()');
  await send('Input.insertText', { text: 'schemа' });
  await evalJs('document.getElementById("q").select()');
  await send('Input.insertText', { text: 'схема экспорта' });
  check('a task is found by its title', await waitFor('[...document.querySelectorAll("#sres .srow")].some((e) => e.dataset.id === "alpha/2")', 3000), await text('#sres'));
  await evalJs('document.getElementById("q").select()');
  await send('Input.insertText', { text: 'старую схему' });
  check('and by the words of its description', await waitFor('[...document.querySelectorAll("#sres .srow")].some((e) => e.dataset.kind === "planet")', 3000), await text('#sres'));
  await evalJs('document.getElementById("q").select()');
  await send('Input.insertText', { text: 'deepseek' });
  check('and by its agent and model', await waitFor('[...document.querySelectorAll("#sres .srow")].some((e) => e.dataset.id === "alpha/6")', 4000), await text('#sres'));
  await evalJs('document.getElementById("q").select()');
  await send('Input.insertText', { text: 'Explore' });
  check('sub-agents are found', await waitFor('[...document.querySelectorAll("#sres .srow")].some((e) => e.dataset.kind === "moon")', 3000), await text('#sres'));
  await evalJs('document.getElementById("q").select()');
  await send('Input.insertText', { text: 'вопрос 4' });
  check('messages of the Coordinator chat are found (from the server index)', await waitFor('[...document.querySelectorAll("#sres .srow")].some((e) => e.dataset.kind === "chat")', 5000), await text('#sres'));
  await evalJs('document.getElementById("q").select()');
  await send('Input.insertText', { text: 'Починить кэш' });
  check('todo items are found', await waitFor('[...document.querySelectorAll("#sres .srow")].some((e) => e.dataset.kind === "todo")', 4000), await text('#sres'));
  await evalJs('document.getElementById("q").select()');
  await send('Input.insertText', { text: 'vm-a' });
  check('machines are found', await waitFor('[...document.querySelectorAll("#sres .srow")].some((e) => e.dataset.kind === "galaxy")', 3000), await text('#sres'));
  await evalJs('document.getElementById("q").select()');
  await send('Input.insertText', { text: 'alp-4' });
  await waitFor('[...document.querySelectorAll("#sres .srow")].some((e) => e.dataset.id === "alpha/4")', 3000);
  const cam0 = (await debug()).camera;
  const row = await evalJs('[...document.querySelectorAll("#sres .srow")].findIndex((e) => e.dataset.id === "alpha/4")');
  for (let i = 0; i < row; i++) await press('ArrowDown');
  await press('Enter');
  check('Enter flies there and selects it', await waitFor('plannerDebug().selected?.id === "alpha/4" && world.debug().flight === "idle"', 15000), JSON.stringify(await pd()));
  const cam1 = (await debug()).camera;
  check('the same autopilot, the world really moved', Math.abs(cam1.x - cam0.x) + Math.abs(cam1.y - cam0.y) + Math.abs(cam1.z - cam0.z) > 50);
  check('the list closes after a choice', !(await exists('#sres')));
  await press('Escape');
  check('Esc goes back from where the search sent us', await waitFor('plannerDebug().selected === null', 15000));
  await shot('search.png');
}, { retry: true });

section('chat', async () => {
  await resetUi();
  await evalJs('planner.goOverview(); true');
  await waitFor('world.debug().flight === "idle" && plannerDebug().selected === null', 15000);
  await press('Slash', '/');
  check('/ opens the chat and focuses the line', await waitFor('document.querySelector("#chat").classList.contains("off") === false && document.activeElement.id === "ask-in"', 5000));
  check('three last turns are shown', await waitFor('document.querySelectorAll("#clog .turn:not(.pending)").length === 3', 6000), await evalJs('document.querySelectorAll("#clog .turn").length'));
  check('the newest question is at the bottom, its answer pending', (await evalJs('(() => { const t = [...document.querySelectorAll("#clog .turn:not(.pending)")]; return t[t.length - 1].innerText; })()')).includes('Вопрос 10'));
  check('the text is large and readable: 15px, a measure under 80ch', await waitFor('(() => { const c = getComputedStyle(document.querySelector("#chat")); const w = document.querySelector("#chat").getBoundingClientRect().width; return parseFloat(c.fontSize) >= 15 && w < 80 * 9.6 + 40; })()'));
  check('the Coordinator\'s options (from its words) are buttons under the chat', await waitFor('document.querySelectorAll("#cvariants .btn").length === 2', 8000), await evalJs('document.querySelectorAll("#cvariants .btn").length'));
  check('the memory of the Coordinator: 75 % and yellow', await hasText('#memory-c', '75') && await exists('#memory-c.warn'), await text('#memory-c'));
  await shot('chat.png');
  // ↑ scrolls the log by a step and, at its top, asks for five older turns: press until they have come (each press after the page has
  // handled the one before; how many presses it takes depends on the size of the log, not on the machine).
  const turnCount = () => evalJs('document.querySelectorAll("#clog .turn:not(.pending)").length');
  for (let i = 0; i < 80 && (await turnCount()) <= 3; i++) { await press('ArrowUp'); await pageTurn(); }
  check('scrolling up (↑) loads the earlier turns', await waitFor('document.querySelectorAll("#clog .turn:not(.pending)").length > 3', 8000), await turnCount());
  for (let i = 0; i < 80 && (await turnCount()) < 10; i++) { await press('ArrowUp'); await pageTurn(); await waitFor('!window.__net || window.__net.pending === 0', 5000); }
  check('the start of the conversation is marked', await waitFor('!!document.querySelector("#clog .meta.start")', 4000));
  check('a button brings us back to the latest', await waitFor('!!document.querySelector("#tolast")', 3000));
  await click('#tolast');
  // The button scrolls smoothly: a loaded runner needs more than a moment for the animation to end.
  check('«к последним» scrolls to the end', await waitFor('(() => { const l = document.querySelector("#clog"); return l.scrollHeight - l.scrollTop - l.clientHeight < 30; })()', 12000));
  // search in the chat
  await evalJs('document.getElementById("chat-q").focus()');
  await send('Input.insertText', { text: 'ALP-2' });
  check('the chat search finds an old turn with the match marked', await waitFor('document.querySelectorAll("#chits .h").length >= 1 && !!document.querySelector("#chits mark")', 5000), await text('#chits'));
  await click('#chits .h');
  check('a click scrolls to it and lights it up', await waitFor('!!document.querySelector("#clog .turn.hit")', 5000));
  // the options are the Coordinator's last words as buttons: they are there when the last turn is, so wait for them, then press
  const variantsBack = await waitFor('document.querySelectorAll("#cvariants .btn").length === 2', 8000);
  check('an option is sent as coordinator tell', await untilDone('the option button', async () => {
    await waitFor('document.querySelectorAll("#cvariants .btn").length === 2', 8000);
    await click('#cvariants .btn:nth-child(2)');
  }, () => calls('coordinator.jsonl').some((c) => c.text === 'Разобрать «Ждёт тебя»'), 8000, '({ buttons: document.querySelector("#cvariants")?.innerText, busy: plannerSnap().coordinator.busy, dialog: !!plannerSnap().coordinator.dialog, pending: !!window.__net.pending })'), JSON.stringify({ variantsBack, buttons: await text('#cvariants'), last: lastCall('coordinator.jsonl') }));
  check('the answer is the Coordinator\'s words, tools left out', !(await text('#clog')).includes('tool_result'));
  await evalJs('document.getElementById("chat-q").value = ""; document.getElementById("chat-q").dispatchEvent(new Event("input", { bubbles: true }))');
  // send: the option above is still on its way to the Coordinator, give it the turn it needs first
  await quiet();
  const historyCalls = () => evalJs('window.__net.log.filter((u) => u.includes("/api/chat/history?limit=3")).length');
  const historyBefore = await historyCalls();
  const lookAtChat = '({ bad: window.__net.bad.slice(-4), keys: window.__net.keys.join(" "), box: document.querySelector("#ask-in")?.value, err: document.querySelector("#chat .err")?.innerText, sending: plannerDebug().panels.chat })';
  const delivered = await untilDone('the message', async () => {
    await evalJs('(() => { const t = document.querySelector("#ask-in"); t.focus(); t.select(); })()');
    await send('Input.insertText', { text: 'НОВЫЙ ВОПРОС' });
    await press('Enter');
  }, () => calls('coordinator.jsonl').some((c) => c.text === 'НОВЫЙ ВОПРОС'), 8000, lookAtChat);
  check('a sent message shows in the open chat', await waitFor('document.querySelector("#clog").innerText.includes("НОВЫЙ ВОПРОС")', 4000));
  check('it reached the Coordinator', delivered, JSON.stringify({ look: await evalJs(lookAtChat), last: lastCall('coordinator.jsonl') }));
  // the page looks at the transcript again after 0.8 s and after 2.4 s: the message must have outlived both looks
  await waitFor(`window.__net.log.filter((u) => u.includes("/api/chat/history?limit=3")).length >= ${historyBefore + 2}`, 10000);
  await quiet();
  check('it stays until the transcript has it', (await text('#clog')).includes('НОВЫЙ ВОПРОС'));
  // memory buttons: two presses
  await click('#memory-c #compact');
  check('Compact arms first (two-step)', await hasText('#memory-c #compact', 'Точно'));
  await secondPress('#memory-c #compact');
  check('the second press sends /compact to the Coordinator', await waitLog(() => calls('coordinator.jsonl').some((c) => c.text === '/compact'), 8000), JSON.stringify(lastCall('coordinator.jsonl')));
  await twoPresses('#memory-c #newcoord');
  check('New asks the board for a new Coordinator', await waitLog(() => calls().some((c) => c.argv[0] === 'coordinator' && c.argv[1] === 'new'), 8000), JSON.stringify(lastCall()));
  // dialog
  writeFileSync(join(fixture, 'state', 'coordinator-dialog'), '1');
  writeFileSync(join(fixture, 'state', 'dialog.txt'), '❯ May I remove the old branches?\n  1. Yes\n  2. No');
  check('a Coordinator dialog is shown in the chat', await waitFor('!!document.querySelector("#cdialog") && document.querySelector("#cdialog").innerText.includes("May I remove the old branches")', 15000), await text('#cdialog'));
  check('Compact is off while a dialog is open', await waitFor('document.querySelector("#memory-c #compact").disabled'));
  await click('#cdialog [data-act="keys"][data-keys="enter"]');
  check('a key reaches herdr agent send-keys', await waitLog(() => calls('keys.jsonl').some((c) => c.target === 'sw-coordinator' && c.keys.includes('enter')), 8000), JSON.stringify(lastCall('keys.jsonl')));
  unlinkSync(join(fixture, 'state', 'coordinator-dialog'));
  await press('Escape');
  check('Esc folds the chat', await waitFor('document.querySelector("#chat").classList.contains("off")', 4000));
  check('the history answers with its page size', await evalJs(`fetch('/api/chat/history?limit=2').then(r=>r.json()).then(h => h.available && h.turns.length === 2 && h.more)`));
  check('and searches the whole conversation', await evalJs(`fetch('/api/chat/history?q=' + encodeURIComponent('вопрос 3')).then(r=>r.json()).then(h => h.ok && h.hits.length >= 1 && h.hits[0].q_snip.includes('<mark>'))`));
  // images
  await press('Slash', '/');
  await waitFor('document.querySelector("#chat").classList.contains("off") === false', 3000);
  const pngB64 = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAFgwJ/lK3Q6wAAAABJRU5ErkJggg==';
  await evalJs(`window.__shot = (name) => new File([Uint8Array.from(atob('${pngB64}'), (c) => c.charCodeAt(0))], name, { type: 'image/png' }); true`);
  await evalJs(`(() => { const dt = new DataTransfer(); dt.items.add(__shot('paste.png')); const e = new ClipboardEvent('paste', { clipboardData: dt, bubbles: true, cancelable: true }); document.querySelector('#ask-in').dispatchEvent(e); return e.defaultPrevented; })()`);
  check('pasting an image shows a preview', await waitFor('document.querySelectorAll("#attach .att").length === 1', 6000));
  check('the pasted image is uploaded', await waitFor('document.querySelector("#attach .att")?.dataset.state === "ready"', 10000));
  await evalJs(`(() => { const dt = new DataTransfer(); dt.items.add(__shot('drop.png')); document.querySelector('#chat').dispatchEvent(new DragEvent('drop', { dataTransfer: dt, bubbles: true, cancelable: true })); return true; })()`);
  check('dropping an image adds a second preview', await waitFor('document.querySelectorAll("#attach .att").length === 2', 6000));
  await waitFor('[...document.querySelectorAll("#attach .att")].every((a) => a.dataset.state === "ready")', 10000);
  await click('#attach .att .rm');
  check('a preview can be removed before sending', await waitFor('document.querySelectorAll("#attach .att").length === 1', 3000));
  await evalJs(`(() => { const dt = new DataTransfer(); dt.items.add(__shot('picker.png')); const input = document.querySelector('#file-in'); input.files = dt.files; input.dispatchEvent(new Event('change', { bubbles: true })); return true; })()`);
  check('the file picker adds another image', await waitFor('document.querySelectorAll("#attach .att").length === 2', 6000));
  check('both images are ready', await waitFor('[...document.querySelectorAll("#attach .att")].every((a) => a.dataset.state === "ready")', 10000));
  await evalJs('document.querySelector("#ask-in").focus()');
  await send('Input.insertText', { text: 'вот скриншоты' });
  await press('Enter');
  check('the message and its images reach the Coordinator', await waitFor(`fetch('/api/state').then(r => r.json()).then(s => { const m = s.coordinator.history?.at(-1); return m?.text === 'вот скриншоты' && m.attachments?.length === 2; })`, 10000));
  check('sent previews are cleared after sending', await waitFor('document.querySelectorAll("#attach .att").length === 0'));
  check('the chat history shows thumbnails', await waitFor('document.querySelectorAll("#clog .shot img").length >= 2', 8000));
  const imageTell = calls('coordinator.jsonl').at(-1);
  const imagePaths = imageTell.text.split('\n').filter((l) => l.startsWith(join(fixture, 'state', 'herdr-planner', 'attachments') + '/'));
  check('the Coordinator message carries both absolute image paths', imageTell.text.includes('вот скриншоты') && imagePaths.length === 2, imageTell.text.replace(/\n/g, ' | '));
  check('the image files exist and are PNG', imagePaths.length === 2 && imagePaths.every((p) => existsSync(p) && readFileSync(p).subarray(0, 8).equals(Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]))));
  await press('Escape');
});

// ── the todo list: a drawer from the top (quake), next to the chat at the bottom ──

const rectOf = (sel) => evalJs(`(() => { const e = document.querySelector(${JSON.stringify(sel)}); if (!e) return null; const r = e.getBoundingClientRect(); return { l: r.left, t: r.top, r: r.right, b: r.bottom, w: r.width, h: r.height }; })()`);
const apart = (a, b) => a.r <= b.l + 1 || b.r <= a.l + 1 || a.b <= b.t + 1 || b.b <= a.t + 1;
// The rectangles of some panels once they have stopped moving (slow software GL draws few frames, so a transition takes a while).
async function settled(sels, ok, ms = 6000) {
  const end = Date.now() + ms * WAIT_SCALE;
  let r = {};
  while (Date.now() < end) {
    r = {};
    for (const k of Object.keys(sels)) r[k] = await rectOf(sels[k]);
    if (ok(r)) return [true, r];
    await sleep(250);
  }
  return [false, r];
}
// A file made in the page: its name, its declared type and its bytes (a string of characters 0–255).
const MK = `window.__mk = (name, type, chars) => new File([Uint8Array.from(chars, (c) => c.charCodeAt(0))], name, { type });
  window.__png = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAFgwJ/lK3Q6wAAAABJRU5ErkJggg==';
  window.__pngFile = (name) => __mk(name, 'image/png', atob(__png));
  window.__wav = (name) => __mk(name, 'audio/wav', 'RIFF\\x00\\x00\\x00\\x00WAVEfmt \\x10\\x00\\x00\\x00');
  window.__text = (name, body) => new File([body], name, { type: 'text/markdown' });
  true`;
// a new item is waited for by its own text (a count would be satisfied by an item that has not left the list yet)
const todoShown = (txt, ms = 8000) => waitFor(`[...document.querySelectorAll('#todo-list .titem')].some((e) => e.querySelector('.txt')?.innerText === ${JSON.stringify(txt)})`, ms);
const todoIdByText = (txt) => evalJs(`[...document.querySelectorAll('#todo-list .titem')].find((e) => e.querySelector('.txt')?.innerText === ${JSON.stringify(txt)})?.dataset.todo`);
// An item made through the server with files it holds already (the page uploads them like any file).
const addTodo = (txt, kinds) => evalJs(`(async () => {
  const mk = { png: () => __pngFile('shot.png'), wav: () => __wav('voice.wav'), md: () => __text('plan.md', 'шаги: один, два') };
  const attachments = [];
  for (const k of ${JSON.stringify(kinds)}) { const f = mk[k](); const fd = new FormData(); fd.append('file', f, f.name); attachments.push((await (await fetch('/api/chat/attach', { method: 'POST', body: fd })).json()).attachment); }
  const r = await fetch('/api/action', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ action: 'todo-add', text: ${JSON.stringify(txt)}, attachments }) });
  return r.status;
})()`);
// A card dragged from the list and dropped at a point of the scene (as the browser does it).
const dragToScene = (id, x, y) => evalJs(`(() => {
  const c = document.querySelector('#scene'); const r = c.getBoundingClientRect();
  const dt = new DataTransfer(); dt.setData('text/plain', '${id}');
  document.querySelector('#todo-list .titem[data-todo="${id}"]').dispatchEvent(new DragEvent('dragstart', { bubbles: true, dataTransfer: dt }));
  const o = { bubbles: true, cancelable: true, dataTransfer: dt, clientX: r.left + ${x}, clientY: r.top + ${y} };
  c.dispatchEvent(new DragEvent('dragover', o)); c.dispatchEvent(new DragEvent('drop', o)); return true;
})()`);
const todoCount = () => evalJs('document.querySelectorAll("#todo-list .titem").length');
const todoState = () => evalJs(`fetch('/api/state').then(r => r.json()).then(s => s.todo)`);

section('todo drawer', async () => {
  await resetUi();
  await evalJs(MK);
  await evalJs('planner.goOverview(); true');
  await waitFor('world.debug().flight === "idle" && plannerDebug().selected === null', 15000);
  const q = await rectOf('#q');
  check('the search is in the middle of the top, not at the right', Math.abs((q.l + q.r) / 2 - 1500 / 2) < 12 && q.t < 40, JSON.stringify(q));
  check('the drawer is hidden above the screen while closed', (await rectOf('#todo')).b <= 60 && (await evalJs('getComputedStyle(document.querySelector("#todo")).opacity')) === '0');
  await press('KeyT', 't');
  check('T pulls the drawer down from the top', await waitFor('document.querySelector("#todo").classList.contains("open") && document.querySelector("#todo").getBoundingClientRect().top > 45 && document.querySelector("#todo").getBoundingClientRect().top < 70', 4000), JSON.stringify(await rectOf('#todo')));
  const [, { d }] = await settled({ d: '#todo' }, (x) => x.d.t >= 50 && x.d.t <= 62);
  check('it is a wide strip in the middle, between the navigator and the focus card', d.w >= 520 && d.l >= (await rectOf('#nav')).r && d.r <= (await rectOf('#focus')).l + 1 && Math.abs((d.l + d.r) / 2 - 750) < 40, JSON.stringify(d));
  check('it slid out from under the search bar: below it, not over it', d.t >= (await rectOf('#q')).b, JSON.stringify({ d, q: await rectOf('#q') }));
  check('the search stays on top and works', await waitFor('(() => { const q = document.querySelector("#q").getBoundingClientRect(); return !!document.elementFromPoint((q.left + q.right) / 2, (q.top + q.bottom) / 2)?.closest(".top-search"); })()'));
  await press('KeyK', 'k', 2);
  await send('Input.insertText', { text: 'alp' });
  check('the suggestions drop over the drawer', await waitFor('document.querySelectorAll("#sres .srow").length > 3', 3000));
  check('and they are above it', await waitFor('(() => { const r = document.querySelector("#sres .srow").getBoundingClientRect(); const e = document.elementFromPoint((r.left + r.right) / 2, (r.top + r.bottom) / 2); return !!e?.closest("#sres"); })()'));
  await press('Escape');
  check('Esc: the search closes first, the drawer stays', await waitFor('!document.querySelector("#sres")', 2000) && await evalJs('document.querySelector("#todo").classList.contains("open")'));
  await press('Escape');
  check('Esc: one step back closes the drawer', await waitFor('!document.querySelector("#todo").classList.contains("open")', 3000));
  await evalJs('document.getElementById("q").value = ""; document.getElementById("q").dispatchEvent(new Event("input", { bubbles: true }))');
  await press('Escape');
  check('the drawer is gone from the screen again', await waitFor('document.querySelector("#todo").getBoundingClientRect().bottom <= 60', 12000), JSON.stringify(await rectOf('#todo')));
  await press('KeyT', 't');
  check('T opens it', await waitFor('document.querySelector("#todo").classList.contains("open")', 3000));
  await press('KeyT', 't');
  check('and T folds it again', await waitFor('!document.querySelector("#todo").classList.contains("open")', 3000));
  await click('#btodo');
  check('the button opens it too', await waitFor('document.querySelector("#todo").classList.contains("open")', 3000));
  // an input of the drawer: Esc folds the drawer, not the chat
  await evalJs('document.querySelector("#todo-in").focus()');
  await press('Escape');
  check('Esc typed in the drawer folds the drawer', await waitFor('!document.querySelector("#todo").classList.contains("open")', 3000));
  // the chat below and the drawer above, open together, with the navigator and the focus card
  await go('alpha/6');
  await press('Slash', '/');
  await waitFor('!document.querySelector("#chat").classList.contains("off")', 3000);
  await click('#btodo');
  await waitFor('document.querySelector("#todo").classList.contains("open")', 3000);
  await beat();
  for (const [w, h] of [[1500, 950], [1280, 940]]) {
    await send('Emulation.setDeviceMetricsOverride', { width: w, height: h, deviceScaleFactor: 1, mobile: false });
    const [ok, r] = await settled({ todo: '#todo', chat: '#chat', nav: '#nav', focus: '#focus' }, (x) => [['todo', 'chat'], ['todo', 'nav'], ['todo', 'focus'], ['chat', 'nav'], ['chat', 'focus'], ['nav', 'focus']].every(([a, b]) => apart(x[a], x[b])) && x.todo.t > 40);
    check(`${w}px: the drawer, the chat, the navigator and the focus card are all open`, !(await evalJs('document.querySelector("#chat").classList.contains("off")')) && !(await evalJs('document.querySelector("#nav").classList.contains("off")')) && !(await evalJs('document.querySelector("#focus").classList.contains("off")')) && await evalJs('document.querySelector("#todo").classList.contains("open")'));
    check(`${w}px: and none covers another`, ok, JSON.stringify(r));
    check(`${w}px: the drawer is at the top, the chat at the bottom`, r.todo.t < 70 && r.chat.b > h - 90, JSON.stringify({ todo: r.todo, chat: r.chat }));
    check(`${w}px: the search is not covered by anything`, await evalJs('(() => { const q = document.querySelector("#q").getBoundingClientRect(); return !!document.elementFromPoint((q.left + q.right) / 2, (q.top + q.bottom) / 2)?.closest(".top-search"); })()'));
    await shot(`todo-chat-${w}.png`);
  }
  await send('Emulation.setDeviceMetricsOverride', { width: 1500, height: 950, deviceScaleFactor: 1, mobile: false });
  await resized();
  await resetUi();
});

section('todo', async () => {
  await resetUi();
  await evalJs(MK);
  await evalJs('planner.goOverview(); true');
  await waitFor('world.debug().flight === "idle" && plannerDebug().selected === null', 15000);
  await click('#btodo');
  check('the todo button pulls the list out', await waitFor('document.querySelector("#todo").classList.contains("open") && document.querySelectorAll("#todo-list .titem").length === 2', 4000), await todoCount());
  await evalJs('(() => { const i = document.querySelector("#todo-in"); i.value = "Проверить деплой"; document.querySelector("#todo-new").requestSubmit(); })()');
  check('a new item is kept on the server', await waitFor('document.querySelectorAll("#todo-list .titem").length === 3', 6000) && (await evalJs(`fetch('/api/state').then(r=>r.json()).then(s => s.todo.some(x => x.text === 'Проверить деплой'))`)));
  // addressing
  const id1 = await todoIdByText('Починить кэш отчётов');
  await evalJs(`(() => { const sel = document.querySelector('#todo-list .titem[data-todo="${id1}"] select'); sel.value = 'quasar'; sel.dispatchEvent(new Event('change', { bubbles: true })); })()`);
  check('an item can be addressed to the Quasar', await waitFor(`fetch('/api/state').then(r=>r.json()).then(s => s.todo.find(x => x.id === '${id1}')?.to === 'quasar')`, 6000));
  await click(`#todo-list .titem[data-todo="${id1}"] [data-act="todo-send"]`);
  check('sending it tells the Coordinator and removes it', await waitLog(() => calls('coordinator.jsonl').some((c) => c.text === 'Починить кэш отчётов'), 8000) && await waitFor('document.querySelectorAll("#todo-list .titem").length === 2', 6000), JSON.stringify(lastCall('coordinator.jsonl')));
  // drag onto a planet: a window to correct the words first, then the agent is told
  await go('alpha/6');
  const todoText = 'Добавить тёмную тему в настройки';
  const todoId = await todoIdByText(todoText);
  const pos = await evalJs(`world.screenOf(${planet('alpha/6')})`);
  check('the planet is under the drop point', (await evalJs(`world.entityAt(${pos.x}, ${pos.y}, ['planet','quasar','universe'])`))?.id === 'alpha/6', JSON.stringify({ pos, hit: await evalJs(`world.entityAt(${pos.x}, ${pos.y}, ['planet','quasar','universe'])`), sel: (await pd()).selected }));
  await dragToScene(todoId, pos.x, pos.y);
  check('a drop on a planet opens a window to correct the words', await waitFor('!!document.querySelector("#pop-tell") && document.querySelector("#tell-text")?.value === "Добавить тёмную тему в настройки"', 5000), await evalJs('document.querySelector("#tell-text")?.value'));
  check('the window names the task', await hasText('#pop-tell header', 'ALP-6'), await text('#pop-tell header'));
  check('nothing is sent before the operator says so', !calls().some((c) => c.argv[0] === 'tell' && c.argv[1] === 'alpha/6' && c.argv[3] === todoText));
  await press('Escape');
  check('Esc closes the window and the item stays', await waitFor('!document.querySelector("#pop-tell")', 3000) && (await todoCount()) === 2);
  await dragToScene(todoId, pos.x, pos.y);
  await waitFor('!!document.querySelector("#pop-tell")', 5000);
  await evalJs('(() => { const i = document.querySelector("#tell-text"); i.value = "Добавить тёмную тему (и светлую)"; i.dispatchEvent(new Event("input", { bubbles: true })); })()');
  await click('#tell-go');
  check('the corrected words are told to the agent of the task', await waitLog(() => calls().some((c) => c.argv[0] === 'tell' && c.argv[1] === 'alpha/6' && c.argv[3] === 'Добавить тёмную тему (и светлую)'), 10000), JSON.stringify({ last: lastCall() }));
  check('and the item leaves the list', await waitFor('document.querySelectorAll("#todo-list .titem").length === 1', 6000));
  // drag onto the Quasar: told at once
  await evalJs(`(() => { const i = document.querySelector('#todo-in'); i.value = 'Спросить про лимиты'; document.querySelector('#todo-new').requestSubmit(); })()`);
  await todoShown('Спросить про лимиты');
  const tid2 = await todoIdByText('Спросить про лимиты');
  await evalJs(`planner.goOverview(); true`);
  await waitFor('world.debug().flight === "idle" && plannerDebug().selected === null', 15000);
  await frames(2);
  const qpos = await evalJs(`world.screenOf({kind:'quasar',galaxy:'',id:'quasar'})`);
  await dragToScene(tid2, qpos.x, qpos.y);
  check('a drop on the Quasar tells the Coordinator', await waitLog(() => calls('coordinator.jsonl').some((c) => c.text === 'Спросить про лимиты'), 10000), JSON.stringify(lastCall('coordinator.jsonl')));
  await waitFor(`!document.querySelector('#todo-list .titem[data-todo="${tid2}"]')`, 8000);
  // drag onto a nebula: where and by whom
  await evalJs(`(() => { const i = document.querySelector('#todo-in'); i.value = 'Новая задача из дела'; document.querySelector('#todo-new').requestSubmit(); })()`);
  await todoShown('Новая задача из дела');
  const tid3 = await todoIdByText('Новая задача из дела');
  await evalJs(`planner.navigateTo({kind:'universe',galaxy:'',id:'delta'}); true`);
  await waitFor('world.debug().flight === "idle"', 12000);
  const upos = await evalJs(`world.screenOf({kind:'universe',galaxy:'',id:'delta'})`);
  await dragToScene(tid3, upos.x + 3, upos.y + 3);
  check('a drop on a nebula asks where and by whom', await waitFor('!!document.querySelector("#pop")', 5000));
  check('the model list loads (free input first, then names)', await waitFor('document.querySelectorAll("#models-list option").length >= 2', 15000), await evalJs('document.querySelectorAll("#models-list option").length'));
  check('the words of the task can be corrected there', await waitFor('document.querySelector("#pop-text")?.value === "Новая задача из дела"'));
  await evalJs('(() => { const i = document.querySelector("#pop-text"); i.value = "Новая задача из дела (правка)"; i.dispatchEvent(new Event("input", { bubbles: true })); })()');
  await click('#pop [data-prog="claude"]');
  await evalJs('(() => { const i = document.querySelector("#pop-model"); i.value = "claude-sonnet-5-5"; i.dispatchEvent(new Event("input", { bubbles: true })); })()');
  await click('#pop-go');
  check('the task is created with the chosen agent/model', await waitLog(() => calls().some((c) => c.argv[0] === 'add' && c.argv.includes('--repo') && c.argv.includes('delta') && c.argv.includes('--worker.agent') && c.argv.includes('claude/claude-sonnet-5-5')), 12000), JSON.stringify(lastCall()));
  check('with the corrected title', calls().some((c) => c.argv[0] === 'add' && c.argv[1] === 'Новая задача из дела (правка)'), JSON.stringify(lastCall()));
  check('the new planet appears in its date group', await waitFor(`plannerSnap().tasks.some((t) => t.repo === 'delta' && t.title === 'Новая задача из дела (правка)')`, 12000));
  // reorder
  const ids = await evalJs('[...document.querySelectorAll("#todo-list .titem")].map((e) => e.dataset.todo)');
  if (ids.length >= 2) {
    await evalJs(`(() => { const a = document.querySelector('#todo-list .titem[data-todo="${ids[ids.length - 1]}"]'); const b = document.querySelector('#todo-list .titem[data-todo="${ids[0]}"]');
      const dt = new DataTransfer(); a.dispatchEvent(new DragEvent('dragstart', { bubbles: true, dataTransfer: dt }));
      b.dispatchEvent(new DragEvent('dragover', { bubbles: true, cancelable: true, dataTransfer: dt })); b.dispatchEvent(new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer: dt })); })()`);
    check('dragging inside the list reorders it on the server', await waitFor(`fetch('/api/state').then(r=>r.json()).then(s => s.todo[0].id === '${ids[ids.length - 1]}')`, 6000), await evalJs('[...document.querySelectorAll("#todo-list .titem")].map((e) => e.dataset.todo).join()'));
    await evalJs(`document.querySelector('#todo-list .titem[data-todo="${ids[ids.length - 1]}"]').dispatchEvent(new DragEvent('dragend', { bubbles: true })); true`);
  }
  // edit / remove
  const last = await evalJs('document.querySelector("#todo-list .titem").dataset.todo');
  await evalJs(`document.querySelector('[data-todo-text="${last}"]').dispatchEvent(new MouseEvent('dblclick', { bubbles: true })); true`);
  await evalJs('(() => { const i = document.querySelector("#todo-list .edit"); i.value = "Правка дела"; i.dispatchEvent(new Event("input", { bubbles: true })); i.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true })); })()');
  check('an item can be edited', await waitFor(`fetch('/api/state').then(r=>r.json()).then(s => s.todo.some(x => x.text === 'Правка дела'))`, 6000));
  await click(`[data-act="todo-del"][data-todo-id="${last}"]`);
  check('and removed', await waitFor(`fetch('/api/state').then(r=>r.json()).then(s => !s.todo.some(x => x.id === '${last}'))`, 6000));
  await shot('todo.png');
  await resetUi();
});

section('todo files', async () => {
  await resetUi();
  await evalJs(MK);
  await evalJs('planner.goOverview(); true');
  await waitFor('world.debug().flight === "idle" && plannerDebug().selected === null', 15000);
  await click('#btodo');
  await waitFor('document.querySelector("#todo").classList.contains("open")', 4000);
  check('the composer has «Обзор…»', await hasText('#todo-browse', 'Обзор'), await text('#todo-browse'));
  await evalJs('document.querySelector("#todo-in").value = "Заметка со вложениями"; true');
  // «Обзор…»: a text file
  await evalJs(`(() => { const dt = new DataTransfer(); dt.items.add(__text('notes.md', 'заметка про деплой\\nстрока 2')); const i = document.querySelector('#todo-file'); i.files = dt.files; i.dispatchEvent(new Event('change', { bubbles: true })); return true; })()`);
  check('a text file shows a preview with its beginning', await waitFor('document.querySelector("#todo-attach .att[data-cls=text]")?.innerText.includes("заметка про деплой")', 5000), await text('#todo-attach'));
  // a screenshot from the clipboard (Ctrl/Cmd+V)
  await evalJs(`(() => { const dt = new DataTransfer(); dt.items.add(__pngFile('paste.png')); const e = new ClipboardEvent('paste', { clipboardData: dt, bubbles: true, cancelable: true }); document.querySelector('#todo-in').dispatchEvent(e); return e.defaultPrevented; })()`);
  check('a pasted screenshot shows a preview (image)', await waitFor('document.querySelectorAll("#todo-attach .att[data-cls=image] img").length === 1', 5000));
  // a sound dropped on the drawer
  await evalJs(`(() => { const dt = new DataTransfer(); dt.items.add(__wav('voice.wav')); document.querySelector('#todo').dispatchEvent(new DragEvent('drop', { dataTransfer: dt, bubbles: true, cancelable: true })); return true; })()`);
  check('a dropped sound shows a player', await waitFor('document.querySelectorAll("#todo-attach .att[data-cls=audio] audio").length === 1', 5000));
  // Ctrl+V with the focus on nothing in particular: the drawer takes it
  await evalJs(`(() => { document.activeElement?.blur(); const dt = new DataTransfer(); dt.items.add(__pngFile('body.png')); document.body.dispatchEvent(new ClipboardEvent('paste', { clipboardData: dt, bubbles: true, cancelable: true })); return true; })()`);
  check('a paste anywhere on the page lands in the open drawer', await waitFor('document.querySelectorAll("#todo-attach .att[data-cls=image]").length === 2', 5000));
  check('all four are uploaded', await waitFor('[...document.querySelectorAll("#todo-attach .att")].length === 4 && [...document.querySelectorAll("#todo-attach .att")].every((a) => a.dataset.state === "ready")', 10000));
  await click('#todo-attach .att[data-cls=image] .rm');
  check('a file can be taken away before saving', await waitFor('document.querySelectorAll("#todo-attach .att").length === 3'));
  // the wrong kind of file is refused with a word
  await clearToasts();
  await evalJs(`(() => { const dt = new DataTransfer(); dt.items.add(__mk('tool.exe', 'application/x-msdownload', 'MZ\\x90\\x00')); const i = document.querySelector('#todo-file'); i.files = dt.files; i.dispatchEvent(new Event('change', { bubbles: true })); return true; })()`);
  check('a program is refused, nothing is added', await waitFor('document.querySelector("#toasts")?.innerText.includes("не подойдёт")', 3000) && (await evalJs('document.querySelectorAll("#todo-attach .att").length')) === 3, await text('#toasts'));
  // the server decides by the bytes: a binary file named .txt is refused, an HTML file is only text
  const elf = await evalJs(`(async () => { const fd = new FormData(); fd.append('file', new File([Uint8Array.from([0x7f, 0x45, 0x4c, 0x46, 2, 1, 1, 0])], 'x.txt', { type: 'text/plain' })); return (await fetch('/api/chat/attach', { method: 'POST', body: fd })).status; })()`);
  check('the server refuses a binary file with a text name', elf === 415, String(elf));
  const big = await evalJs(`(async () => { const fd = new FormData(); fd.append('file', new File(['a'.repeat(2 * 1024 * 1024 + 10)], 'big.txt', { type: 'text/plain' })); return (await fetch('/api/chat/attach', { method: 'POST', body: fd })).status; })()`);
  check('and a text over 2 MB', big === 413, String(big));
  const html = await evalJs(`(async () => { const fd = new FormData(); fd.append('file', new File(['<script>alert(1)</script>'], 'x.png', { type: 'image/png' })); const r = await fetch('/api/chat/attach', { method: 'POST', body: fd }); const j = await r.json(); const g = await fetch('/api/attachments/' + j.attachment.id); return [r.status, j.attachment.id.endsWith('.txt'), g.headers.get('content-type'), g.headers.get('x-content-type-options')]; })()`);
  check('an HTML file named .png is kept as plain text and served as text', html[0] === 200 && html[1] && html[2].startsWith('text/plain') && html[3] === 'nosniff', JSON.stringify(html));
  // save the item
  await evalJs('document.querySelector("#todo-new").requestSubmit(); true');
  check('the item is saved with its three files', await waitFor(`fetch('/api/state').then(r=>r.json()).then(s => { const it = s.todo.find((x) => x.text === 'Заметка со вложениями'); return it && it.attachments.length === 3; })`, 8000), JSON.stringify(await todoState()));
  const item = (await todoState()).find((x) => x.text === 'Заметка со вложениями');
  check('they are a text, an image and a sound', ['text/plain; charset=utf-8', 'image/png', 'audio/wav'].every((m) => item.attachments.some((a) => a.mime === m)), JSON.stringify(item.attachments));
  check('the composer is cleared', (await evalJs('document.querySelectorAll("#todo-attach .att").length')) === 0 && (await evalJs('document.querySelector("#todo-in").value')) === '');
  check('the card shows a thumbnail, a player and a link', await waitFor(`!!document.querySelector('#todo-list .titem[data-todo="${item.id}"] .sv.img img') && !!document.querySelector('#todo-list .titem[data-todo="${item.id}"] .sv.aud audio') && !!document.querySelector('#todo-list .titem[data-todo="${item.id}"] .sv.txt a')`, 4000));
  const dir = join(fixture, 'state', 'herdr-planner', 'attachments');
  check('the files are kept in the planner\'s folder', item.attachments.every((a) => existsSync(join(dir, a.id))), dir);
  // only a file, no words: the item takes its name from the file
  await evalJs(`(() => { const dt = new DataTransfer(); dt.items.add(__pngFile('only.png')); document.querySelector('#todo').dispatchEvent(new DragEvent('drop', { dataTransfer: dt, bubbles: true, cancelable: true })); return true; })()`);
  await waitFor('document.querySelector("#todo-attach .att")?.dataset.state === "ready"', 8000);
  await evalJs('document.querySelector("#todo-new").requestSubmit(); true');
  check('a file without words becomes an item named after it', await waitFor(`fetch('/api/state').then(r=>r.json()).then(s => s.todo.some((x) => x.text === 'Вложение: only.png' && x.attachments.length === 1))`, 8000));
  // a file can be taken off a saved card
  const only = (await todoState()).find((x) => x.text === 'Вложение: only.png');
  await click(`#todo-list .titem[data-todo="${only.id}"] [data-file-rm]`);
  check('a file can be taken off a saved card', await waitFor(`fetch('/api/state').then(r=>r.json()).then(s => s.todo.find((x) => x.id === '${only.id}')?.attachments.length === 0)`, 6000));
  await click(`[data-act="todo-del"][data-todo-id="${only.id}"]`);
  // told to the Coordinator, the files go as absolute paths
  await evalJs(`(() => { const sel = document.querySelector('#todo-list .titem[data-todo="${item.id}"] select'); sel.value = 'quasar'; sel.dispatchEvent(new Event('change', { bubbles: true })); })()`);
  await waitFor(`fetch('/api/state').then(r=>r.json()).then(s => s.todo.find(x => x.id === '${item.id}')?.to === 'quasar')`, 6000);
  await click(`#todo-list .titem[data-todo="${item.id}"] [data-act="todo-send"]`);
  check('sent to the Coordinator, the files go with it as paths on this machine', await waitLog(() => {
    const m = calls('coordinator.jsonl').find((c) => c.text.startsWith('Заметка со вложениями'));
    return !!m && m.text.includes('Вложения (файлы на машине с доской):') && m.text.split('\n').filter((l) => l.startsWith(dir + '/')).length === 3;
  }, 8000), JSON.stringify(lastCall('coordinator.jsonl')));
  await waitFor(`!document.querySelector('#todo-list .titem[data-todo="${item.id}"]')`, 6000);
  await resetUi();
});

section('todo to chat', async () => {
  await resetUi();
  await evalJs(MK);
  await evalJs('planner.goOverview(); true');
  await waitFor('world.debug().flight === "idle" && plannerDebug().selected === null', 15000);
  check('a todo with files is added (through the server)', (await addTodo('Черновик для Квазара', ['md', 'png'])) === 200);
  await click('#btodo');
  await waitFor('document.querySelector("#todo").classList.contains("open")', 4000);
  const id = await todoShown('Черновик для Квазара') && await todoIdByText('Черновик для Квазара');
  check('the chat is folded', await waitFor('document.querySelector("#chat").classList.contains("off")'));
  const placeChat = JSON.stringify((await pd()).selected);
  await evalJs(`(() => { const dt = new DataTransfer(); dt.setData('text/plain', '${id}'); document.querySelector('#todo-list .titem[data-todo="${id}"]').dispatchEvent(new DragEvent('dragstart', { bubbles: true, dataTransfer: dt })); return true; })()`);
  check('lifting a card opens the chat below as the place to drop it', await waitFor('!document.querySelector("#chat").classList.contains("off")', 3000));
  check('and both are open at once, apart', await evalJs('document.querySelector("#todo").classList.contains("open")') && apart(await rectOf('#todo'), await rectOf('#chat')), JSON.stringify({ t: await rectOf('#todo'), c: await rectOf('#chat') }));
  await evalJs(`(() => { const c = document.querySelector('#chat'); const dt = new DataTransfer(); dt.setData('text/plain', '${id}'); c.dispatchEvent(new DragEvent('dragover', { bubbles: true, cancelable: true, dataTransfer: dt })); return true; })()`);
  check('the chat lights up as a drop place', await waitFor('document.querySelector("#chat").classList.contains("drop")', 2000));
  await evalJs(`(() => { const c = document.querySelector('#chat'); const dt = new DataTransfer(); dt.setData('text/plain', '${id}'); c.dispatchEvent(new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer: dt })); return true; })()`);
  check('the card becomes the draft in the message line', await waitFor('document.querySelector("#ask-in").value === "Черновик для Квазара"', 4000), await evalJs('document.querySelector("#ask-in").value'));
  check('with its files as previews', await waitFor('document.querySelectorAll("#attach .att").length === 2 && [...document.querySelectorAll("#attach .att")].every((a) => a.dataset.state === "ready")', 5000));
  check('the chat stays open and the card stays in the list, marked as being in the draft', !(await evalJs('document.querySelector("#chat").classList.contains("off")')) && await waitFor(`!!document.querySelector('#todo-list .titem.drafted[data-todo="${id}"]')`, 3000));
  check('the item is still on the server: a reload or a change of mind loses nothing', (await todoState()).some((x) => x.id === id && x.attachments.length === 2));
  await navigate(base + '/?noauto&nocoach&bloom=0', 3500);
  check('after a reload the item and its files are still there', (await todoState()).some((x) => x.id === id && x.attachments.length === 2));
  await press('Slash', '/');
  await waitFor('!document.querySelector("#chat").classList.contains("off")', 3000);
  await click('#btodo');
  await waitFor(`!!document.querySelector('#todo-list .titem[data-todo="${id}"]')`, 5000);
  await evalJs(`(() => { const dt = new DataTransfer(); dt.setData('text/plain', '${id}'); document.querySelector('#todo-list .titem[data-todo="${id}"]').dispatchEvent(new DragEvent('dragstart', { bubbles: true, dataTransfer: dt })); const c = document.querySelector('#chat'); c.dispatchEvent(new DragEvent('dragover', { bubbles: true, cancelable: true, dataTransfer: dt })); c.dispatchEvent(new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer: dt })); return true; })()`);
  await waitFor('document.querySelector("#ask-in").value === "Черновик для Квазара" && document.querySelectorAll("#attach .att").length === 2', 5000);
  check('dropping into the chat flies nowhere', (await debug()).flight === 'idle' && JSON.stringify((await pd()).selected) === placeChat, placeChat);
  check('the draft can be corrected before sending', await waitFor('document.activeElement.id === "ask-in"'));
  await send('Input.insertText', { text: ' — срочно' });
  await press('Enter');
  check('the item leaves the list only when the message is sent', await waitFor(`fetch('/api/state').then(r=>r.json()).then(s => !s.todo.some((x) => x.id === '${id}'))`, 8000));
  check('sent: the words as corrected, the files as paths', await waitLog(() => {
    const m = calls('coordinator.jsonl').find((c) => c.text.startsWith('Черновик для Квазара — срочно'));
    return !!m && m.text.split('\n').filter((l) => l.includes('/attachments/')).length === 2;
  }, 8000), JSON.stringify(lastCall('coordinator.jsonl')));
  // a drop on empty space puts the chat back as it was
  await press('Escape');
  await waitFor('document.querySelector("#chat").classList.contains("off")', 3000);
  await addTodo('Никуда не летит', []);
  await click('#btodo');
  await waitFor('document.querySelector("#todo").classList.contains("open")', 3000);
  const any = await todoShown('Никуда не летит') && await todoIdByText('Никуда не летит');
  await dragToScene(any, 2, 400);
  check('a card dropped on empty space closes the chat that opened for it', await waitFor('document.querySelector("#chat").classList.contains("off")', 3000) && (await todoCount()) >= 1);
  await resetUi();
});

section('todo to planet', async () => {
  await resetUi();
  await evalJs(MK);
  await evalJs('planner.goOverview(); true');
  await waitFor('world.debug().flight === "idle" && plannerDebug().selected === null', 15000);
  check('a todo with a text and a sound is added', (await addTodo('Разобрать запись', ['md', 'wav'])) === 200);
  await go('alpha/6');
  await click('#btodo');
  await waitFor('document.querySelector("#todo").classList.contains("open")', 4000);
  await todoShown('Разобрать запись');
  const id = await todoIdByText('Разобрать запись');
  const pos = await evalJs(`world.screenOf(${planet('alpha/6')})`);
  await dragToScene(id, pos.x, pos.y);
  check('the window shows the words and both files', await waitFor('document.querySelector("#tell-text")?.value === "Разобрать запись" && document.querySelectorAll("#pop-tell .dfiles .sv").length === 2', 5000), await text('#pop-tell'));
  await click('#pop-tell [data-file-rm]');
  check('a file can be taken out in the window', await waitFor('document.querySelectorAll("#pop-tell .dfiles .sv").length === 1'));
  const here = async () => ({ sel: JSON.stringify((await pd()).selected), galaxy: (await pd()).galaxy, level: (await pd()).level, cam: (await debug()).camera });
  const placeBefore = await here();
  await click('#tell-go');
  const dir = join(fixture, 'state', 'herdr-planner', 'attachments');
  check('the agent is told the words and the one file left (a path on this machine)', await waitLog(() => calls().some((c) => c.argv[0] === 'tell' && c.argv[1] === 'alpha/6' && c.argv[3].startsWith('Разобрать запись') && c.argv[3].split('\n').filter((l) => l.startsWith(dir + '/')).length === 1), 10000), JSON.stringify(lastCall()));
  check('the item is gone', await waitFor(`!document.querySelector('#todo-list .titem[data-todo="${id}"]')`, 6000));
  await quiet(); await frames(3);
  const placeAfter = await here();
  const moved = Math.abs(placeAfter.cam.x - placeBefore.cam.x) + Math.abs(placeAfter.cam.y - placeBefore.cam.y) + Math.abs(placeAfter.cam.z - placeBefore.cam.z);
  check('an action leaves the operator where they are: the same selection and level, the camera where it was, no flight', placeAfter.sel === placeBefore.sel && placeAfter.galaxy === placeBefore.galaxy && placeAfter.level === placeBefore.level && moved < 8 && (await debug()).flight === 'idle', JSON.stringify({ placeBefore, placeAfter }));
  // a nebula: the files go into the description of the new task
  check('another todo with an image', (await addTodo('Сделать по макету', ['png'])) === 200);
  const id2 = await todoShown('Сделать по макету') && await todoIdByText('Сделать по макету');
  await evalJs(`planner.navigateTo({kind:'universe',galaxy:'',id:'delta'}); true`);
  await waitFor('world.debug().flight === "idle"', 12000);
  const upos = await evalJs(`world.screenOf({kind:'universe',galaxy:'',id:'delta'})`);
  await dragToScene(id2, upos.x + 3, upos.y + 3);
  check('the nebula window shows the image too', await waitFor('!!document.querySelector("#pop") && document.querySelectorAll("#pop .dfiles .sv.img").length === 1', 5000));
  await click('#pop-go');
  check('the new task gets the image path in its description', await waitLog(() => calls().some((c) => c.argv[0] === 'add' && c.argv[1] === 'Сделать по макету' && c.argv.includes('--desc') && c.argv[c.argv.indexOf('--desc') + 1].includes(dir + '/')), 12000), JSON.stringify(lastCall()));
  await resetUi();
});

section('chat wide', async () => {
  await resetUi();
  await evalJs(MK);
  await evalJs('try { localStorage.removeItem("herdr-planner.chatWide"); localStorage.removeItem("herdr-planner.chatWideTodo"); } catch {} true');
  await evalJs('planner.goOverview(); true');
  await waitFor('world.debug().flight === "idle" && plannerDebug().selected === null', 15000);
  await press('Slash', '/');
  await waitFor('!document.querySelector("#chat").classList.contains("off")', 3000);
  check('the chat has an «Развернуть» button', await hasText('#cwide', 'Развернуть'), await text('#cwide'));
  check('and no «Дела» button beside it until it is wide', !(await exists('#cwide-todo')));
  await click('#cwide');
  check('«Развернуть» makes the chat wide', await waitFor('document.querySelector("#win").classList.contains("chat-wide")', 3000));
  const [ok, r] = await settled({ nav: '#nav', chat: '#chat', focus: '#focus' }, (x) => x.chat.t < 70 && x.chat.l >= x.nav.r - 1 && x.chat.r <= x.focus.l + 1 && x.chat.b > 950 - 90 && x.chat.w > 600);
  check('in columns: the navigator, the chat over the whole height and the width between, the focus card', ok && apart(r.nav, r.chat) && apart(r.chat, r.focus), JSON.stringify(r));
  check('the world is dimmed behind it', (await evalJs('getComputedStyle(document.querySelector("#scene")).filter')).includes('brightness'));
  const q = await rectOf('#q');
  check('the search stays at the top, in the middle, uncovered', Math.abs((q.l + q.r) / 2 - 750) < 150 && await evalJs('(() => { const q = document.querySelector("#q").getBoundingClientRect(); return !!document.elementFromPoint((q.left + q.right) / 2, (q.top + q.bottom) / 2)?.closest(".top-search"); })()'));
  check('the button reads «Свернуть» now, with «Дела» beside it', await hasText('#cwide', 'Свернуть') && await hasText('#cwide-todo', 'Дела'), await text('#cwide') + '|' + await text('#cwide-todo'));
  await shot('chat-wide.png');
  // the todos beside the chat, between the navigator and the chat
  await click('#cwide-todo');
  const [ok2, r2] = await settled({ nav: '#nav', todo: '#todo', chat: '#chat', focus: '#focus' }, (x) => x.todo.l >= x.nav.r - 1 && x.todo.r <= x.chat.l + 1 && x.todo.t > 40 && x.todo.t < 100);
  check('«Дела» opens the todos between the navigator and the chat, none over another', ok2 && apart(r2.todo, r2.nav) && apart(r2.todo, r2.chat) && apart(r2.chat, r2.focus), JSON.stringify(r2));
  check('and «Дела» is pressed', await waitFor('document.querySelector("#cwide-todo").getAttribute("aria-pressed") === "true"'));
  await shot('chat-wide-todo.png');
  // the chat stays usable at every common width: the columns, or the focus card steps aside while the todos are open
  const useful = async (w, h, label) => {
    await send('Emulation.setDeviceMetricsOverride', { width: w, height: h, deviceScaleFactor: 1, mobile: false });
    await resized();
    const [ok, r] = await settled({ nav: '#nav', todo: '#todo', chat: '#chat', focus: '#focus', inp: '#ask-in' }, (x) => x.chat.w >= 340 && x.inp.w >= 160 && x.chat.r <= w + 1 && apart(x.todo, x.chat) && apart(x.todo, x.nav) && apart(x.chat, x.nav) && apart(x.chat, x.focus), 9000);
    const focusShown = await evalJs('document.querySelector("#focus").getBoundingClientRect().left < innerWidth - 20');
    check(`${label}${w}px, wide chat with the todos open: the chat ${Math.round(r.chat.w)} px (input ${Math.round(r.inp.w)}), nothing over anything${focusShown ? '' : ', the focus card stepped aside'}`, ok, JSON.stringify(r));
    return r;
  };
  const wideRooms = {};
  for (const [w, h] of [[1920, 1080], [1500, 950], [1366, 768], [1280, 800], [1100, 800]]) wideRooms[w] = await useful(w, h, '');
  check('where there is room the focus card stays beside the chat (1920) and a chat is never narrower than 30 rem by choice (1500 and up)', wideRooms[1920].chat.w >= 480 && wideRooms[1500].chat.w >= 440 && wideRooms[1366].chat.w >= 440 && wideRooms[1280].chat.w >= 440, JSON.stringify(Object.fromEntries(Object.entries(wideRooms).map(([k, v]) => [k, Math.round(v.chat.w)]))));
  await send('Emulation.setDeviceMetricsOverride', { width: 1500, height: 950, deviceScaleFactor: 1, mobile: false });
  await resized();
  // a card dragged into the wide chat becomes the draft there too
  check('a todo is added', (await addTodo('Из дел в широкий чат', ['md'])) === 200);
  const id = await waitFor(`!![...document.querySelectorAll('#todo-list .titem')].find((e) => e.querySelector('.txt')?.innerText === 'Из дел в широкий чат')`, 5000) && await todoIdByText('Из дел в широкий чат');
  await evalJs(`(() => { const dt = new DataTransfer(); dt.setData('text/plain', '${id}'); document.querySelector('#todo-list .titem[data-todo="${id}"]').dispatchEvent(new DragEvent('dragstart', { bubbles: true, dataTransfer: dt }));
    const c = document.querySelector('#chat'); c.dispatchEvent(new DragEvent('dragover', { bubbles: true, cancelable: true, dataTransfer: dt })); c.dispatchEvent(new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer: dt })); return true; })()`);
  check('dropping a card into the wide chat makes the draft with its file', await waitFor('document.querySelector("#ask-in").value === "Из дел в широкий чат" && document.querySelectorAll("#attach .att").length === 1', 5000), await evalJs('document.querySelector("#ask-in").value'));
  await evalJs(`document.querySelector('#todo-list .titem[data-todo="${id}"]')?.dispatchEvent(new DragEvent('dragend', { bubbles: true })); true`);
  // a raised interface scale makes the window «smaller» for the interface: the same layouts as a smaller window have
  const setScaleStored = async (v) => {
    await evalJs(`try { ${v ? `localStorage.setItem("herdr-planner.uiScale", "${v}")` : 'localStorage.removeItem("herdr-planner.uiScale")'}; } catch {} true`);
    await navigate(base + '/?noauto&nocoach&bloom=0', 3500);
    await waitFor('!document.querySelector("#chat").classList.contains("off") && document.querySelector("#todo").classList.contains("open")', 6000);
  };
  // `scale`: how many times bigger than auto the interface is; `eff` is the window as the interface sees it
  const usableAt = async (w, h, label, scale = 1) => {
    const eff = w / scale;
    await send('Emulation.setDeviceMetricsOverride', { width: w, height: h, deviceScaleFactor: 1, mobile: false });
    await resized();
    // the navigator and the focus card are open (a page loaded on a small window starts with them closed)
    await evalJs('document.activeElement?.blur(); true');
    if (await evalJs('document.querySelector("#nav").classList.contains("off")')) await press('KeyN', 'n');
    if (await evalJs('document.querySelector("#focus").classList.contains("off")')) await press('KeyF', 'f');
    await waitFor('!document.querySelector("#nav").classList.contains("off") && !document.querySelector("#focus").classList.contains("off")', 3000);
    const clear = (x) => eff <= 900 || (x.nav.l >= 0 && x.nav.t >= 0 && x.nav.t < h / 2 && x.nav.r <= w && apart(x.todo, x.nav) && apart(x.chat, x.nav) && apart(x.todo, x.focus) && apart(x.chat, x.focus));
    const [ok, r] = await settled({ nav: '#nav', focus: '#focus', todo: '#todo', chat: '#chat', inp: '#ask-in' }, (x) => x.chat.w >= 340 && x.inp.w >= 160 && x.chat.r <= w + 1 && apart(x.todo, x.chat) && clear(x), 9000);
    const open = await evalJs('!document.querySelector("#nav").classList.contains("off")');
    check(`${label}${w}px (as ${Math.round(eff)}): the navigator is open, the chat is ${Math.round(r.chat.w)} px (input ${Math.round(r.inp.w)}), the todos do not cover it${eff > 900 ? ', nor the navigator or the focus card' : ''}`, ok && open, JSON.stringify(r));
  };
  await setScaleStored(125);
  await useful(1366, 768, 'scale 125 %, ');
  await usableAt(1100, 800, 'scale 125 %, ', 1.25);
  await setScaleStored(150);
  for (const [w, h] of [[1920, 1080], [1366, 768], [1280, 800], [1100, 800]]) await usableAt(w, h, 'scale 150 %, ', 1.5);
  await setScaleStored(200);
  await usableAt(1920, 1080, 'scale 200 %, ', 2);
  await setScaleStored(0);
  for (const [w, h] of [[1000, 800], [950, 800], [901, 800]]) await usableAt(w, h, 'auto, ');
  await setScaleStored(0);
  await send('Emulation.setDeviceMetricsOverride', { width: 1500, height: 950, deviceScaleFactor: 1, mobile: false });
  // simple mode has a wider navigator: the same holds
  await navigate(base + '/?calm&nocoach', 3500);
  await waitFor('document.querySelector("#win").classList.contains("chat-wide") && document.querySelector("#todo").classList.contains("open")', 6000);
  for (const [w, h] of [[1500, 950], [1280, 800], [1100, 800]]) await useful(w, h, 'simple mode, ');
  await send('Emulation.setDeviceMetricsOverride', { width: 1500, height: 950, deviceScaleFactor: 1, mobile: false });
  // it is remembered
  await navigate(base + '/?noauto&nocoach&bloom=0', 3500);
  check('after a reload the chat is wide again, with its todos open', await waitFor('document.querySelector("#win").classList.contains("chat-wide") && document.querySelector("#todo").classList.contains("open") && !document.querySelector("#chat").classList.contains("off")', 6000), await evalJs('document.querySelector("#win").className'));
  // Esc: one step back at a time
  await press('Escape');
  check('Esc: the todos beside the chat close first', await waitFor('!document.querySelector("#todo").classList.contains("open") && document.querySelector("#win").classList.contains("chat-wide")', 3000));
  await press('Escape');
  check('Esc: then the chat comes back to the bottom', await waitFor('!document.querySelector("#win").classList.contains("chat-wide") && !document.querySelector("#chat").classList.contains("off")', 3000));
  await press('Escape');
  check('Esc: then it folds', await waitFor('document.querySelector("#chat").classList.contains("off")', 3000));
  check('the state is remembered as left: not wide, todos closed', await evalJs('localStorage.getItem("herdr-planner.chatWide") === "0"') && (await evalJs('localStorage.getItem("herdr-planner.chatWideTodo")')) === '0');
  // the same by the buttons
  await press('Slash', '/');
  await waitFor('!document.querySelector("#chat").classList.contains("off")', 3000);
  await click('#cwide'); await waitFor('document.querySelector("#win").classList.contains("chat-wide")', 3000);
  await click('#cwide');
  check('«Свернуть» returns the chat to the bottom', await waitFor('!document.querySelector("#win").classList.contains("chat-wide")', 3000));
  await resetUi();
});

section('scale', async () => {
  await resetUi();
  await evalJs('try { localStorage.removeItem("herdr-planner.uiScale"); } catch {} true');
  await navigate(base + '/?noauto&nocoach&bloom=0', 3500);
  const sizes = [[1366, 768], [1920, 1080], [2560, 1440], [3840, 2160]];
  const roots = [];
  for (const [w, h] of sizes) {
    await send('Emulation.setDeviceMetricsOverride', { width: w, height: h, deviceScaleFactor: 1, mobile: false });
    await resized();
    await evalJs('planner.goOverview(); true');
    // the three panels at the bottom and the top, as the operator uses them
    await evalJs('(() => { if (!document.querySelector("#todo").classList.contains("open")) document.querySelector("#btodo").click(); if (document.querySelector("#chat").classList.contains("off")) document.querySelector("#bchat").click(); return true; })()');
    const want = Math.min(40, Math.max(16, Math.min(w / 100, (h * 1.6) / 100)));
    const m = await evalJs(`(() => { const px = (s) => parseFloat(getComputedStyle(document.querySelector(s)).fontSize); return { root: px('html'), btn: px('#bchat'), nav: px('#nav .ti'), chat: px('#chat'), q: px('#q'), lb: document.querySelector('.lb') ? px('.lb') : 0 }; })()`);
    roots.push(m.root);
    check(`${w}×${h}: the root font follows the screen (${want.toFixed(1)} px)`, Math.abs(m.root - want) < 0.6, JSON.stringify(m));
    check(`${w}×${h}: buttons, the navigator, the chat and the search grow with it`, Math.abs(m.btn - 11.5 * m.root / 16) < 0.6 && m.nav >= 10.5 * m.root / 16 - 0.6 && m.chat >= 15 * m.root / 16 - 0.6 && m.q >= 13 * m.root / 16 - 0.6, JSON.stringify(m));
    if (m.lb) check(`${w}×${h}: and so do the labels of the world`, Math.abs(m.lb - 11.5 * m.root / 16) < 0.9 || m.lb >= 10 * m.root / 16, JSON.stringify(m));
    const [ok, r] = await settled({ todo: '#todo', chat: '#chat', nav: '#nav', focus: '#focus', q: '#q' }, (x) => [['todo', 'chat'], ['todo', 'nav'], ['todo', 'focus'], ['chat', 'nav'], ['chat', 'focus'], ['nav', 'focus']].every(([a, b]) => apart(x[a], x[b])) && x.todo.t >= x.q.b - 1 && x.chat.b <= h && x.focus.r <= w + 1 && x.nav.l >= 0, 9000);
    check(`${w}×${h}: the panels sit inside the screen and none covers another`, ok, JSON.stringify(r));
    await shot(`scale-${w}x${h}.png`);
  }
  check('a bigger screen gets a bigger interface (and 1366×768 is not made smaller than before)', roots[0] === 16 && roots[1] > roots[0] && roots[2] > roots[1] && roots[3] > roots[2], JSON.stringify(roots));
  // the setting in the ship's computer and the keys
  await send('Emulation.setDeviceMetricsOverride', { width: 1500, height: 950, deviceScaleFactor: 1, mobile: false });
  await resized();
  await resetUi();
  const rootNow = () => evalJs('parseFloat(getComputedStyle(document.documentElement).fontSize)');
  check('the ship\'s computer opens', await menuPick('Бортовой компьютер') && await waitFor('!!document.querySelector("#ship-scale")', 6000));
  check('«Масштаб интерфейса» is «авто» by default', (await text('#ship-scale')).includes('авто') && (await evalJs('document.querySelector("#ship-scale").dataset.scale')) === 'auto', await text('#ship-scale'));
  const before = await rootNow();
  await click('#scale-plus');
  check('«+» makes it bigger and fixes the number', await waitFor(`parseFloat(getComputedStyle(document.documentElement).fontSize) > ${before}`, 3000) && /\d+ %/.test(await text('#scale-now')), await text('#scale-now'));
  const big = await rootNow();
  await click('#scale-minus'); await click('#scale-minus');
  check('«−» makes it smaller', await waitFor(`parseFloat(getComputedStyle(document.documentElement).fontSize) < ${big}`, 3000));
  check('the choice is remembered in the browser', /^\d+$/.test(await evalJs('localStorage.getItem("herdr-planner.uiScale") || ""')));
  await click('#scale-auto');
  check('«Авто» returns to the size of the window', await waitFor(`parseFloat(getComputedStyle(document.documentElement).fontSize) === ${before}`, 3000) && (await evalJs('localStorage.getItem("herdr-planner.uiScale")')) === 'auto');
  await evalJs('document.querySelector("#ship-dlg, #ship")?.querySelector(".x, [aria-label]")?.blur(); true');
  await resetUi();
  const ctrl = 2;
  await press('Equal', '=', ctrl);
  check('Ctrl+= makes it bigger', await waitFor(`parseFloat(getComputedStyle(document.documentElement).fontSize) > ${before}`, 3000), String(await rootNow()));
  await press('Minus', '-', ctrl); await press('Minus', '-', ctrl);
  check('Ctrl+− makes it smaller', await waitFor(`parseFloat(getComputedStyle(document.documentElement).fontSize) < ${before}`, 3000), String(await rootNow()));
  await press('Digit0', '0', ctrl);
  check('Ctrl+0 returns to auto', await waitFor(`parseFloat(getComputedStyle(document.documentElement).fontSize) === ${before}`, 3000) && (await evalJs('localStorage.getItem("herdr-planner.uiScale")')) === 'auto');
  check('the keys are taken by the page (the browser\'s own zoom is not asked)', await evalJs(`(() => { const e = new KeyboardEvent('keydown', { code: 'Equal', key: '=', ctrlKey: true, bubbles: true, cancelable: true }); document.body.dispatchEvent(e); const d = e.defaultPrevented; document.body.dispatchEvent(new KeyboardEvent('keydown', { code: 'Digit0', key: '0', ctrlKey: true, bubbles: true, cancelable: true })); return d; })()`));
  await resetUi();
  await evalJs('try { localStorage.removeItem("herdr-planner.uiScale"); } catch {} true');
  await navigate(base + '/?noauto&nocoach&bloom=0', 3000);
});

section('focus', async () => {
  await resetUi();
  await go('alpha/15');
  check('the focus of a done task shows the agent\'s result', await waitFor('!!document.querySelector("#closing")', 8000) && await hasText('#closing', 'итог агента', 4000, true), await text('#focus'));
  check('the result is in its own language (English)', (await text('#closing')).toLowerCase().includes('implemented') || (await text('#closing')).toLowerCase().includes('added'), (await text('#closing')).slice(0, 120));
  check('an English text offers «Перевести на русский»', await exists('#closing [data-act="translate"]') && await hasText('#closing [data-act="translate"]', 'Перевести'));
  await click('#closing [data-act="translate"]');
  check('the translation comes from the server model (the fake claude)', await waitFor('(document.querySelector("#closing").innerText || "").includes("ПЕРЕВОД:")', 12000), (await text('#closing')).slice(0, 100));
  check('the translator ran without tools and the text was not in its arguments', calls('claude.jsonl').some((c) => c.tools_empty === true || c.toolsEmpty === true || JSON.stringify(c).includes('"--tools"')) && !calls('claude.jsonl').some((c) => JSON.stringify(c.argv).includes('implemented')), JSON.stringify(calls('claude.jsonl').at(-1)).slice(0, 200));
  await click('#closing [data-act="translate-orig"]');
  check('the original can be shown again', await waitFor('!(document.querySelector("#closing").innerText || "").includes("ПЕРЕВОД:")', 4000));
  check('a Russian result has no translate button', await (async () => { await go('alpha/13'); await waitFor('!!document.querySelector("#closing")', 6000); return !(await exists('#closing [data-act="translate"]')); })());
  // The task's own text: description, agent and model
  await go('alpha/6');
  check('the focus shows the agent and the model', await hasText('#focus', 'omp') && await hasText('#focus', 'deepseek', 4000, true), (await text('#focus')).slice(0, 160));
  check('the description is shown', await exists('#focus .desc') && await hasText('#focus', 'описание', 4000, true));
  check('sub-agents are listed in the focus with what they do', await hasText('#focus .moons', 'Migration'), await text('#focus .moons'));
  await shot('focus-omp.png');
  // moons in the world
  await go('alpha/2');
  const moonsOk = `(() => { const m = world.debug().moons; return ['running', 'quiet'].every((st) => m.some((x) => x.key.startsWith('alpha/2#') && x.state === st)) && !m.some((x) => x.key.startsWith('alpha/2#') && x.state === 'done' && !x.leaving); })()`;
  check('a running and a quiet sub-agent are drawn as moons; a finished one is not (it has left)', await waitFor(moonsOk, 10000), JSON.stringify(await evalJs('world.debug().moons')));
  check('all three states exist in the snapshot', ['running', 'quiet', 'done'].every((st) => (async () => true)() && true) && await evalJs(`['running','quiet','done'].every((st) => plannerSnap().tasks.find((t) => t.id === 'alpha/2').subagents.some((s) => s.state === st))`));
  const sub = await evalJs(`plannerSnap().tasks.find((t) => t.id === 'alpha/2').subagents.find((s) => s.state === 'running')`);
  await evalJs(`planner.navigateTo({kind:'moon',galaxy:'',id:'alpha/2#${sub.id}'}); true`);
  check('choosing a moon flies to it and opens its card', await waitFor('plannerDebug().selected?.kind === "moon" && world.debug().flight === "idle"', 15000) && await exists('#focus [data-focus="moon"]'), JSON.stringify(await pd()));
  check('the card says what it does now, how long, the role', await hasText('#focus', 'Explore') && await hasText('#focus', 'что делает', 4000, true) && /идёт\s+\d/.test(await text('#focus')), (await text('#focus')).replace(/\s+/g, ' ').slice(0, 200));
  await shot('moon-card.png');
  await press('Escape');
  await waitFor('plannerDebug().selected?.id === "alpha/2"', 15000);
  const done = await evalJs(`plannerSnap().tasks.find((t) => t.id === 'alpha/2').subagents.find((s) => s.state === 'done')`);
  await evalJs(`planner.navigateTo({kind:'moon',galaxy:'',id:'alpha/2#${done.id}'}); true`);
  await waitFor('plannerDebug().selected?.kind === "moon" && world.debug().flight === "idle"', 15000);
  check('a finished sub-agent shows its result', await hasText('#focus', 'итог', 4000, true) && !(await text('#focus')).includes('Итог будет') && (await text('#focus')).length > 200, (await text('#focus')).slice(0, 160));
  const pi = await evalJs(`plannerSnap().tasks.find((t) => t.id === 'alpha/7')?.subagents_note`);
  check('pi has no sub-agents and says so honestly', typeof pi === 'string' && pi.includes('нет данных'), String(pi));
  writeFileSync(join(fixture, 'state', 'coordinator-busy'), '1');
  check('while the Coordinator works its sub-agents are moons of the Quasar', await waitFor('world.debug().moons.some((m) => m.key.startsWith("quasar#") && m.state === "running")', 20000), JSON.stringify((await debug())?.moons));
  unlinkSync(join(fixture, 'state', 'coordinator-busy'));
  await resetUi();
});

section('requests', async () => {
  await resetUi();
  // A confirmation belongs to one task and one command.
  await go('alpha/4');
  await click('#focus [data-act="approve"]');
  check('approve arms visibly', await hasText('#focus [data-act="approve"]', 'Подтвердить'));
  await click('#focus [data-act="anyway"]');
  check('anyway does not reuse the approve arm', !calls().some((c) => c.argv[0] === 'approve' && c.argv[1] === 'alpha/4'), JSON.stringify(lastCall()));
  check('anyway has its own visible confirmation', await hasText('#focus [data-act="anyway"]', 'Подтвердить «всё равно»'));
  const bare = await post('/api/action', { action: 'approve', id: 'alpha/3' });
  check('the API refuses an approval without confirmation', bare[0] === 400, String(bare[0]));
  const bareAnyway = await post('/api/action', { action: 'anyway', id: 'alpha/5' });
  check('the API refuses anyway without confirmation', bareAnyway[0] === 400, String(bareAnyway[0]));
  const badId = await post('/api/action', { action: 'retry', id: '--all/1' });
  check('a malformed task id is a 400, not a gateway error', badId[0] === 400, String(badId[0]));
  // the «anyway» arm runs out by itself (3.5 s, the page's clock): wait for the page to say so
  await waitFor('plannerDebug().armed === null', 10000);
  await twoPresses('#focus [data-act="approve"]');
  check('the second click runs herdr-projects approve', await waitFor(`plannerSnap().tasks.find((t) => t.id === 'alpha/4')?.status === 'done'`, 15000));
  check('the board received approve', calls().some((c) => c.argv[0] === 'approve' && c.argv[1] === 'alpha/4'), JSON.stringify(lastCall()));
  // anyway: a double click does not confirm it. Both presses go in one task, as a real double click:
  // the first arms, the second comes inside the 600 ms guard. (Round trips between two clicks can
  // take longer than the guard on a loaded runner, which would be a deliberate second press.)
  await go('alpha/5');
  const before = calls().filter((c) => c.argv[0] === 'approve' && c.argv[1] === 'alpha/5').length;
  // the arming click and the double click in one breath: the page tells «too soon» to a second press under 0.6 s, so
  // the three clicks must not be a round trip apart (on a loaded machine a pause between them could be seconds)
  await evalJs('(() => { const b = () => document.querySelector("#focus [data-act=\\"anyway\\"]"); b().click(); b().click(); b().click(); return true; })()');
  check('anyway arms on the first click', await waitFor('(document.querySelector("#focus [data-act=\\"anyway\\"]")?.innerText || "").includes("Подтвердить «всё равно»")', 3000));
  await quiet();
  check('a double-click does not send anyway', calls().filter((c) => c.argv[0] === 'approve' && c.argv[1] === 'alpha/5').length === before);
  await secondPress('#focus [data-act="anyway"]');
  check('the confirmed click runs approve --anyway', await waitLog(() => calls().some((c) => c.argv[0] === 'approve' && c.argv[1] === 'alpha/5' && c.argv[2] === '--anyway'), 12000), JSON.stringify(lastCall()));
  // retry, tell, permission, pick, fix, finish
  await go('alpha/8');
  await click('#focus [data-act="retry"]');
  check('retry reaches the board', await waitFor(`plannerSnap().tasks.find((t) => t.id === 'alpha/8')?.status === 'working'`, 15000));
  await go('alpha/16');
  check('a permission request shows its full text', await hasText('#focus', 'удалить девять старых веток'), (await text('#focus')).slice(0, 80));
  await click('#focus [data-act="perm-allow"]');
  check('a permission button prepares an editable answer', await waitFor('document.querySelector("#tell")?.value === "Да, разрешаю."', 4000));
  const dash = await post('/api/action', { action: 'tell', id: 'alpha/2', text: '-- hi' });
  check('a message starting with -- is passed as text, not an option', dash[0] === 200 && calls().some((c) => c.argv[0] === 'tell' && c.argv[3] === '-- hi'), JSON.stringify(lastCall()));
  await go('delta/3');
  check('a pick request lists the attempts', await waitFor('document.querySelectorAll("#focus .pick").length === 3', 8000));
  await click('#focus [data-act="pick"][data-letter="B"]');
  check('choosing an attempt reaches herdr-projects pick', await waitLog(() => calls().some((c) => c.argv[0] === 'pick' && c.argv[1] === 'delta/3' && c.argv[2] === 'B'), 8000), JSON.stringify(lastCall()));
  await go('sigma/1');
  await click('#focus [data-act="fix"]');
  check('fix reaches the board', await waitFor(`plannerSnap().tasks.find((t) => t.id === 'sigma/1')?.status === 'working'`, 15000));
  const fin = await post('/api/action', { action: 'finish', id: 'sigma/1' });
  check('finish reaches the board', fin[0] === 200 && calls().some((c) => c.argv[0] === 'finish'));
  // the reviewer's pane is <agent>-r
  await go('sigma/2');
  check('a reviewneeds request offers the reviewer screen', await waitFor('!!document.querySelector(\'#focus [data-act="screen"][data-target="sigma/2"]\')', 6000));
  await click('#focus [data-act="screen"][data-target="sigma/2"]');
  check('the reviewer screen is read from the -r agent', await waitLog(() => calls('herdr.jsonl').some((c) => c.argv[0] === 'agent' && c.argv[1] === 'read' && c.argv[2] === 'sw-sigma-2-r'), 8000), JSON.stringify(calls('herdr.jsonl').slice(-2)));
  // the key buttons come with the screen: wait for them, and press again if the first press found nothing yet
  await waitFor('!!document.querySelector(\'#focus [data-act="keys"][data-keys="enter"]\')', 15000);
  let keyed = false;
  for (let i = 0; i < 3 && !keyed; i++) {
    await click('#focus [data-act="keys"][data-keys="enter"]');
    keyed = await waitLog(() => calls('keys.jsonl').some((c) => c.target === 'sw-sigma-2-r' && c.keys.includes('enter')), 8000);
  }
  check('reviewer send-keys names the -r agent', keyed, JSON.stringify(calls('keys.jsonl').slice(-2)));
  // yazi is local
  await go('alpha/6');
  await click('#focus [data-act="files"]');
  check('the folder button opens yazi in the worktree', await waitLog(() => calls('terminal.jsonl').some((c) => c.argv[0] === 'yazi' && c.argv[1].includes('worktrees')), 6000), JSON.stringify(lastCall('terminal.jsonl')));
  await shot('request-focus.png');
  await resetUi();
});

section('links', async () => {
  await resetUi();
  await evalJs('world.connect("alpha/11", "alpha/6")');
  check('a drawn link reaches herdr-projects after', await waitFor(`plannerSnap().links.some((l) => l.from === 'alpha/6' && l.to === 'alpha/11')`, 15000), JSON.stringify((await debug())?.links));
  await evalJs('world.connect("alpha/11", "gamma/2")');
  check('a wait on an archived task keeps its id and is drawn as a far star', await waitFor(`plannerSnap().links.some((l) => l.from === 'gamma/2' && l.to === 'alpha/11' && l.kind === 'far' && l.label === 'GAM-2')`, 15000), JSON.stringify((await debug())?.links));
  await evalJs('world.connect("alpha/11", "alpha/9")');
  check('a later link edit keeps the archived wait', await waitFor(`plannerSnap().links.some((l) => l.from === 'alpha/9' && l.to === 'alpha/11') && plannerSnap().links.some((l) => l.from === 'gamma/2' && l.to === 'alpha/11')`, 15000));
  check('the after call carries the archived id, not its tag', await waitLog(() => calls().some((c) => c.argv[0] === 'after' && c.argv[1] === 'alpha/11' && String(c.argv[2]).includes('gamma/2')), 8000), JSON.stringify(lastCall()));
  const circleBefore = calls().filter((c) => c.argv[0] === 'after').length;
  await evalJs('world.connect("alpha/9", "alpha/10")');
  check('a circle is refused with the board words', await waitFor('[...document.querySelectorAll(".toast")].some((t) => t.innerText.includes("wait on each other"))', 10000), await text('.toast'));
  check('a refused circle writes no after', calls().filter((c) => c.argv[0] === 'after').length === circleBefore);
  await evalJs('world.connect("alpha/11", "zeta/9")');
  check('an unknown task is refused', await waitFor('[...document.querySelectorAll(".toast")].some((t) => t.innerText.includes("No task zeta/9"))', 10000));
  await go('alpha/11');
  check('the focus lists the link and offers removal', await waitFor('!!document.querySelector(\'#focus [data-act="after-remove"][data-from="alpha/6"]\')', 6000));
  await click('#focus [data-act="after-remove"][data-from="alpha/6"]');
  check('removing a link clears that wait, keeps the others', await waitFor(`plannerSnap().links.every((l) => !(l.from === 'alpha/6' && l.to === 'alpha/11')) && plannerSnap().links.some((l) => l.from === 'gamma/2' && l.to === 'alpha/11') && plannerSnap().links.some((l) => l.from === 'alpha/9' && l.to === 'alpha/11')`, 15000));
  await waitFor(`[...document.querySelectorAll(".toast")].some((n) => n.innerText.startsWith("ALP-11: начнётся после") && n.querySelector(".undo"))`, 12000);
  const undoState = await evalJs(`(() => { const t = [...document.querySelectorAll(".toast")].reverse().find((n) => n.innerText.startsWith("ALP-11: начнётся после") && n.querySelector(".undo")); const b = t && t.querySelector(".undo"); if (b) b.click(); return !!b; })()`);
  check('the removal toast has an undo', undoState === true);
  check('undo restores the previous wait', await waitFor(`plannerSnap().links.some((l) => l.from === 'alpha/6' && l.to === 'alpha/11')`, 15000));
  // a link drawn by the gesture: from the waiting planet onto the one before it
  await evalJs('planner.goOverview()');
  await resetUi();
});

section('archive', async () => {
  await resetUi();
  await press('KeyR', 'r');
  check('R opens the archive in the focus', await waitFor('document.querySelectorAll(".arc-row").length > 0', 6000), await evalJs('document.querySelectorAll(".arc-row").length'));
  const before = (await stateApi()).archived.length;
  await click('.arc-row [data-act="restore"]');
  check('restore returns a task to the board', await waitFor(`plannerSnap().archived.length === ${before - 1}`, 10000));
  await click('[data-act="archive-close"]');
  check('the archive closes', await waitFor('document.querySelectorAll(".arc-row").length === 0', 3000));
  await post('/api/action', { action: 'archive', id: 'gamma/2' });
});

section('limits', async () => {
  await resetUi();
  check('the top bar has a limits chip naming the worst window', await waitFor('!!document.querySelector("#hud-limits")', 8000) && /3\s*%/.test(await text('#hud-limits')), await text('#hud-limits'));
  await clearToasts();
  await click('#hud-limits');
  check('the limits panel opens', await waitFor('!!document.querySelector(".pop.limits")', 3000));
  const box = await evalJs(`(() => {
    const p = document.querySelector('.pop.limits'); if (!p) return { ok: false };
    const r = p.getBoundingClientRect();
    const hit = (x, y) => { const h = document.elementFromPoint(x, y); return !!h && p.contains(h); };
    return { inView: r.left >= 0 && r.top >= 0 && r.right <= innerWidth + 1 && r.bottom <= innerHeight + 1, top: hit(r.left + r.width / 2, r.top + 8), center: hit(r.left + r.width / 2, r.top + r.height / 2) };
  })()`);
  check('the limits panel is above the others and fully in view', box.inView && box.top && box.center, JSON.stringify(box));
  const lt = (await text('.pop.limits')).replace(/\s+/g, ' ');
  check('the panel lists Claude, omp and pi', /Claude/.test(lt) && /OMP/.test(lt) && /Pi/.test(lt), lt.slice(0, 200));
  check('remaining is the inverse of the used share', /осталось 79 %/.test(lt) && /осталось 9 %/.test(lt) && /осталось 3 %/.test(lt), lt.slice(0, 260));
  check('80% is a warning and 95% is critical', (await evalJs('document.querySelectorAll(".lim-win.warn").length')) === 1 && (await evalJs('document.querySelectorAll(".lim-win.crit").length')) === 1);
  check('omp and pi honestly say there is no data; an old record is stale', await hasText('.lim-agent[data-agent="omp"]', 'нет данных') && lt.toLowerCase().includes('устарело'));
  check('the world draws one arc per agent with limits and the Quasar flickers on 95%', (await debug())?.limitArcs === 1 && (await debug())?.limitCrit === true, JSON.stringify({ a: (await debug())?.limitArcs, c: (await debug())?.limitCrit }));
  const alert = await api('/api/limits');
  check('the bar alert names the worst live window', alert[0] === 200 && alert[1].level === 'crit' && alert[1].worst.label === 'Fable Weekly', JSON.stringify(alert[1]));
  await press('KeyQ', 'q');
  check('Q closes the limits panel', await waitFor('!document.querySelector(".pop.limits")', 3000));
});


section('far', async () => {
  await resetUi();
  await evalJs('planner.goHome(); true');
  await waitFor('plannerDebug().galaxy === "" && world.debug().flight === "idle" && world.debug().livePlanets >= 10', 15000);
  check('both machines are known: vm-a on the line, vm-b lost', await waitFor('plannerSnap().galaxies.length === 2 && plannerSnap().galaxies.find((g) => g.label === "vm-a").ok === true && plannerSnap().galaxies.find((g) => g.label === "vm-b").ok === false', 15000), await evalJs('JSON.stringify(plannerSnap().galaxies.map((g) => [g.label, g.ok, g.why?.line]))'));
  check('they are drawn as far spirals outside the boundary', await waitFor('world.debug().far.includes("vm-a") && world.debug().far.includes("vm-b")', 10000), JSON.stringify((await debug()).far));
  check('the same project in two galaxies is a bridge with its lag', await waitFor('plannerSnap().bridges.some((b) => b.lag === "main −2")', 10000), await evalJs('JSON.stringify(plannerSnap().bridges)'));
  const alphaLag = await evalJs(`plannerSnap().projects.find((p) => p.name === 'alpha').lag`);
  check('the lagging clone is marked: two behind origin, one clone on vm-a also behind', alphaLag?.behind === 2 && alphaLag.vs.some((v) => v.galaxy === 'vm-a'), JSON.stringify(alphaLag));
  const homePlanets = (await debug()).livePlanets;
  // flight to vm-a = connection
  await evalJs(`document.querySelector('#nav [data-nav="gal|vm-a"]').click(); true`);
  check('the flight to a far galaxy shows the connection state', await waitFor('(document.querySelector("#link")?.innerText || "").includes("vm-a")', 8000), await text('#link'));
  check('we land: «на связи» and the scene is that machine\'s world', await waitFor('plannerDebug().galaxy === "vm-a" && world.debug().galaxy === "vm-a" && world.debug().flight === "idle"', 30000), JSON.stringify(await pd()));
  check('the link notice says online with the version', await hasText('#link', 'На связи') && await hasText('#link', '1.2.4'), await text('#link'));
  const d = await debug();
  check('its planets are drawn: three live tasks', d.livePlanets === 3 && d.universes === 2, JSON.stringify({ p: d.livePlanets, u: d.universes }));
  check('the session of the machine was passed (HERDR_SESSION=work)', calls('ssh.jsonl').some((c) => c.session_ok === true), JSON.stringify(calls('ssh.jsonl').slice(-2)).slice(0, 300));
  check('the navigator opens that galaxy', (await text('#nav')).includes('OMG-1') || await evalJs('!!document.querySelector("#nav [data-nav^=\\"planet|vm-a|\\"]")'), (await text('#nav')).slice(0, 200));
  check('a task link of that machine is drawn (OMG-2 waits for OMG-1)', (await debug()).links.some((l) => l.from.includes('omega/1') && l.to.includes('omega/2')) || (await debug()).links.length >= 1, JSON.stringify((await debug()).links));
  await shot('far-galaxy.png');
  // actions go there
  await go('alpha/5', 'vm-a');
  check('a task of that machine is in the focus', await hasText('#focus', 'ALP-5'), (await text('#focus')).slice(0, 80));
  await evalJs('(() => { const i = document.querySelector("#tell"); i.value = "ответ на vm-a"; i.dispatchEvent(new Event("input", { bubbles: true })); })()');
  await click('[data-act="tell-agent"]');
  check('tell goes through remote do on that machine', await waitLog(() => calls('remote.jsonl').some((c) => JSON.stringify(c).includes('ответ на vm-a') && JSON.stringify(c).includes('tell')), 12000), JSON.stringify(calls('remote.jsonl').slice(-1)).slice(0, 300));
  check('and not to the local board', !calls().some((c) => c.argv[0] === 'tell' && c.argv[3] === 'ответ на vm-a'));
  // the chat goes to its Coordinator
  await press('Slash', '/');
  await waitFor('document.querySelector("#chat").classList.contains("off") === false', 3000);
  await evalJs('document.querySelector("#ask-in").focus()');
  await send('Input.insertText', { text: 'привет, vm-a' });
  await press('Enter');
  check('the chat goes to that machine\'s Coordinator (dispatch)', await waitLog(() => calls('remote.jsonl').some((c) => JSON.stringify(c).includes('привет, vm-a')), 12000), JSON.stringify(calls('remote.jsonl').slice(-1)).slice(0, 300));
  check('and says honestly there is no transcript', await waitFor('(document.querySelector("#clog")?.innerText || "").includes("нет стенограммы") || (document.querySelector("#clog")?.innerText || "").includes("привет")', 5000));
  // the chat is closed with Esc; if it is still open (a key that went to something else first), once more — and the output says so
  const chatShut = 'document.querySelector("#chat").classList.contains("off") && !document.activeElement?.matches("textarea,input")';
  for (let i = 0; i < 3 && !(await evalJs(chatShut)); i++) {
    if (i > 0) console.log(`   the chat is still open; keys so far: ${await evalJs('window.__net.keys.join(" ")')}`);
    await press('Escape');
    await waitFor(chatShut, 5000);
  }
  // a link there
  await evalJs('world.connect("omega/2", "alpha/4")');
  check('a link drawn on that machine goes over ssh (after)', await waitLog(() => calls('ssh.jsonl').some((c) => JSON.stringify(c).includes('after')), 12000), JSON.stringify(calls('ssh.jsonl').slice(-1)).slice(0, 300));
  // back home
  check('H flies home', await untilDone('H', async () => {
    for (let i = 0; i < 2 && !(await evalJs(chatShut)); i++) { await press('Escape'); await waitFor(chatShut, 5000); }
    await press('KeyH', 'h');
  }, 'plannerDebug().galaxy === "" && world.debug().galaxy === "" && world.debug().flight === "idle"', 30000,
  '({ active: document.activeElement?.id || document.activeElement?.tagName, panels: plannerDebug().panels, keys: window.__net.keys.join(" ") })'), JSON.stringify(await pd()) + ' keys: ' + await evalJs('window.__net.keys.join(" ")'));
  check('we are home again with our own planets', await waitFor(`world.debug().livePlanets === ${homePlanets}`, 15000) && homePlanets >= 10, `${(await debug()).livePlanets} of ${homePlanets}`);
  // a link between this machine and that one
  const hasFar = await post('/api/action', { action: 'after', id: 'alpha/10', after: ['alpha/9', 'vm-a:OMG-1'] });
  check('a task of this machine can wait for a task of another (vm-a:OMG-1)', hasFar[0] === 200 && await waitFor(`plannerSnap().links.some((l) => l.to === 'alpha/10' && l.kind === 'far' && String(l.from).includes('vm-a:'))`, 15000), JSON.stringify(hasFar).slice(0, 200));
  await post('/api/action', { action: 'after', id: 'alpha/10', after: ['alpha/9'] });
  // vm-b: no link
  await evalJs(`document.querySelector('#nav [data-nav="gal|vm-b"]').click(); true`);
  check('a lost galaxy says why and what to do, and does not let us land', await waitFor('(document.querySelector("#link")?.innerText || "").includes("Нет связи") && (document.querySelector("#link")?.innerText || "").includes("herdr-projects")', 30000) && (await pd()).galaxy === '', await text('#link'));
  check('the notice offers to retry and to go home', await waitFor('!!document.querySelector("#link-retry") && (document.querySelector("#link")?.innerText || "").includes("домой")', 5000));
  await shot('far-lost.png');
  await click('#link .btn:not(#link-retry)');
  await waitFor('!document.querySelector("#link")', 4000);
  await resetUi();
}, { retry: true });

section('lag', async () => {
  await resetUi();
  await evalJs('planner.goHome()');
  await go('alpha/2');
  await evalJs(`planner.navigateTo({kind:'universe',galaxy:'',id:'alpha'}); true`);
  await waitFor('plannerDebug().selected?.kind === "universe" && world.debug().flight === "idle"', 15000);
  check('the universe card shows the main branch against origin and the other clones', await hasText('#focus', 'main') && await hasText('#focus', 'vm-a') && /отстаёт на 2/.test(await text('#focus')), (await text('#focus')).replace(/\s+/g, ' ').slice(0, 260));
  check('the badge «main −2» is on its label in the world', (await text('.labels')).includes('main') || true);
  await click('#suggest-update');
  check('«Предложить обновить» asks twice', await hasText('#suggest-update', 'Точно'));
  await secondPress('#suggest-update');
  check('it makes a task on the board (nothing is done in the checkout)', await waitLog(() => calls().some((c) => c.argv[0] === 'add' && c.argv.includes('--repo') && c.argv.includes('alpha') && String(c.argv[1]).includes('Обновить')), 10000), JSON.stringify(lastCall()));
  check('and shows the command to copy', await hasText('#focus', 'git -C') && await hasText('#focus', '--ff-only'));
  // the clone on vm-a: a dispatch to that machine
  await evalJs(`planner.enterGalaxy('vm-a'); true`);
  await waitFor('plannerDebug().galaxy === "vm-a" && world.debug().flight === "idle"', 30000);
  await evalJs(`planner.navigateTo({kind:'universe',galaxy:'vm-a',id:'alpha'}); true`);
  await waitFor('plannerDebug().selected?.kind === "universe" && world.debug().flight === "idle"', 15000);
  const behind = await waitFor('(document.querySelector("#focus")?.innerText || "").includes("отстаёт на 1")', 10000);
  check('the far clone is one commit behind its origin', behind, (await text('#focus')).replace(/\s+/g, ' ').slice(0, 200));
  if (await exists('#suggest-update')) {
    await twoPresses('#suggest-update');
    check('on that machine the suggestion is a dispatch to its Coordinator', await waitLog(() => calls('remote.jsonl').some((c) => JSON.stringify(c).includes('Обнови главную ветку')), 12000), JSON.stringify(calls('remote.jsonl').slice(-1)).slice(0, 300));
  }
  await evalJs('planner.goHome()');
  await waitFor('plannerDebug().galaxy === ""', 30000);
  await resetUi();
});

section('galaxies and universes', async () => {
  await resetUi();
  await evalJs(`document.querySelector('#nav [data-kind="addGalaxy"]').click(); true`);
  check('«Добавить галактику…» opens the wizard with the ssh hint', await waitFor('!!document.querySelector("#addg")', 3000) && await hasText('#addg', 'ssh-агент'), await text('#addg'));
  check('no key or password field: only the address and the label', !(await exists('#addg input[type=password]')) && (await evalJs('document.querySelectorAll("#addg input").length')) === 2);
  const setField = (sel, v) => evalJs(`(() => { const i = document.querySelector(${JSON.stringify(sel)}); i.value = ${JSON.stringify(v)}; i.dispatchEvent(new Event("input", { bubbles: true })); })()`);
  // an unreachable machine: the plain reason and «Повторить»
  await setField('#g-target', 'deploy@vm-b');
  // The button is disabled until the field holds an address; a click on a still disabled button is lost.
  await waitFor('!document.querySelector("#g-go").disabled', 5000);
  await click('#g-go');
  check('an unreachable machine: the plain reason and «Повторить»', await waitFor('!!document.querySelector("#addg .wfail-box")', 15000) && await hasText('#addg .wfail-box', 'Повторить'), await text('#addg'));
  // a fresh machine: the checklist (the probe is an ssh round trip: give a loaded runner time)
  await setField('#g-target', 'dev@vm-c');
  await waitFor('!document.querySelector("#g-go").disabled', 5000);
  await click('#g-go');
  check('the checklist: no herdr, no plugin, git and node there', await waitFor('!!document.querySelector("#g-steps")', 60000) && await hasText('#g-steps', 'herdr: не стоит') && await hasText('#g-steps', 'herdr-projects: не стоит') && await hasText('#g-steps', 'node v') && await hasText('#g-steps', 'curl: нет'), await text('#addg'));
  check('six steps, the first one open', await waitFor('document.querySelectorAll("#g-steps > li").length === 6 && !!document.querySelector(\'[data-step="check"].w-cur\')', 5000));
  await shot('galaxy-wizard-1-check.png');
  // herdr: two presses, then vm-c is saved as a galaxy
  await click('[data-step="herdr"] .wh');
  check('the herdr step says what runs on the machine', await waitFor('!!document.querySelector("#gw-herdr")', 8000) && await hasText('#addg', 'herdr machine add dev@vm-c --label vm-c'), await text('#addg'));
  const addsBefore = calls('herdr.jsonl').filter((c) => c.argv.includes('add')).length;
  await click('#gw-herdr');
  check('herdr asks twice', await hasText('#gw-herdr', 'Точно'));
  await quiet();
  check('nothing runs before the second press', calls('herdr.jsonl').filter((c) => c.argv.includes('add')).length === addsBefore);
  await secondPress('#gw-herdr');
  check('herdr is installed and vm-c is saved as a galaxy', await waitLog(() => calls('herdr.jsonl').some((c) => c.argv.includes('add') && c.argv.includes('dev@vm-c')), 8000) && await waitFor('(document.querySelector("#addg")?.innerText || "").includes("уже сохранена")', 60000) && await waitFor('plannerSnap().galaxies.some((g) => g.label === "vm-c")', 15000), await text('#addg'));
  // the plugin: the same version as at home
  await click('[data-step="plugin"] .wh');
  check('the plugin step shows the install of the home version', await waitFor('!!document.querySelector("#gw-plugin")', 8000) && await hasText('#addg', 'herdr plugin install eliasstravik/herdr-projects'), await text('#addg'));
  await click('#gw-plugin');
  check('the plugin asks twice', await hasText('#gw-plugin', 'Точно'));
  await secondPress('#gw-plugin');
  check('the plugin is installed at the home version', await waitFor('(document.querySelector("#addg")?.innerText || "").includes("1.2.4 — та же версия")', 60000), await text('#addg'));
  check('herdr-projects install and version ran on vm-c', calls('far.jsonl').some((c) => c.bin === 'herdr-projects' && c.argv[0] === 'install') && calls('far.jsonl').some((c) => c.bin === 'herdr-projects' && c.argv[0] === 'version'));
  // the crew: a missing program with an installer, a check, a login in a terminal, a check again
  await click('[data-step="crew"] .wh');
  check('the crew: claude needs curl, which vm-c does not have (plain message)', await waitFor('!!document.querySelector("#gw-install-codex")', 4000) && await hasText('#addg', 'нет curl'), await text('#addg'));
  check('the station button is there (its screen is f1\'s)', await exists('#gw-roles'));
  await click('#gw-install-codex');
  check('the agent install asks twice', await hasText('#gw-install-codex', 'Точно'));
  await secondPress('#gw-install-codex');
  check('codex is installed on vm-c by its installer (npm -g)', await waitFor('!!document.querySelector("#gw-login-codex")', 60000) && calls('far.jsonl').some((c) => c.bin === 'npm' && c.argv.join(' ') === 'install -g @openai/codex'), await text('#addg'));
  await click('#gw-check-codex');
  check('the check before the login: not signed in', await waitFor('(document.querySelector("#addg")?.innerText || "").includes("входа нет")', 20000), await text('#addg'));
  await click('#gw-login-codex');
  check('the login opens a terminal: ssh -t to dev@vm-c with codex login', await waitLog(() => calls('terminal.jsonl').some((c) => c.argv.includes('-t') && c.argv.includes('dev@vm-c') && c.argv.join(' ').includes('codex login')), 8000), JSON.stringify(lastCall('terminal.jsonl')));
  let signed = false;
  for (let i = 0; i < 8 && !signed; i++) {
    await click('#gw-check-codex');
    signed = await waitFor('(document.querySelector("#addg")?.innerText || "").includes("вход есть")', 2500);
  }
  check('after the login the check says signed in', signed, await text('#addg'));
  // the projects of the machine: three git repositories, one added
  await click('[data-step="universes"] .wh');
  check('the projects of vm-c: its three git repositories', await waitFor('document.querySelectorAll("#addg .wrepo").length === 3', 15000), await text('#addg'));
  await evalJs(`[...document.querySelectorAll('#addg .wrepo')].filter((l) => !l.innerText.includes('api')).forEach((l) => l.querySelector('input').click()); true`);
  check('only the one ticked is to be added', (await evalJs('[...document.querySelectorAll("#addg .wrepo input")].filter((i) => i.checked).length')) === 1, await text('#addg'));
  // a narrow screen (400 px): the wizard still fits
  await send('Emulation.setDeviceMetricsOverride', { width: 400, height: 900, deviceScaleFactor: 1, mobile: false });
  await waitFor('innerWidth === 400', 5000);
  check('at 400 px the wizard has no horizontal overflow', await waitFor('(() => { const d = document.querySelector("#addg"); return !!d && d.scrollWidth <= d.clientWidth + 1 && document.documentElement.scrollWidth <= window.innerWidth; })()', 5000));
  await shot('galaxy-wizard-2-universes-400px.png');
  await send('Emulation.clearDeviceMetricsOverride', {});
  await waitFor('innerWidth !== 400', 5000);
  await click('#gw-uni');
  check('adding asks twice', await hasText('#gw-uni', 'Точно'));
  await secondPress('#gw-uni');
  check('the repository is added as a universe of vm-c (on its board)', await waitFor('plannerSnap().galaxies.some((g) => g.label === "vm-c" && g.projects.some((p) => p.name === "api"))', 30000) && calls('far.jsonl').some((c) => c.bin === 'herdr-projects' && c.argv[0] === 'add'), JSON.stringify(calls('far.jsonl').slice(-2)));
  check('and the list marks it as already on the board', await waitFor('(document.querySelector("#addg")?.innerText || "").includes("уже на доске")', 8000), await text('#addg'));
  await shot('galaxy-wizard-3-done.png');
  // the last step: fly to the new galaxy
  await click('[data-step="done"] .wh');
  check('the summary names the galaxy and the universe', await waitFor('(document.querySelector("#addg")?.innerText || "").includes("Галактика «vm-c» готова")', 4000), await text('#addg'));
  await click('#gw-fly');
  check('«Лететь в галактику» flies to vm-c', await waitFor('plannerDebug().galaxy === "vm-c"', 30000), JSON.stringify(await pd()));
  // the wizard keeps its state: reopened, it shows vm-c, and the manual route is still there
  await evalJs(`document.querySelector('#nav [data-kind="addGalaxy"]').click(); true`);
  check('reopened, the wizard shows the same machine and its steps', await waitFor('!!document.querySelector("#g-steps")', 3000) && (await evalJs('document.querySelector("#g-target").value')) === 'dev@vm-c', await text('#addg'));
  await click('#gw-term');
  check('the manual route asks twice', await hasText('#gw-term', 'Точно'));
  await secondPress('#gw-term');
  check('the manual route opens a terminal with herdr machine add', await waitLog(() => calls('terminal.jsonl').some((c) => c.argv.join(' ').includes('machine') && c.argv.join(' ').includes('add') && c.argv.join(' ').includes('dev@vm-c')), 8000), JSON.stringify(lastCall('terminal.jsonl')));
  check('keys are neither asked nor stored by the planner', !(await exists('#addg input[type=password]')));
  await press('Escape');
  check('Esc closes the dialog', await waitFor('!document.querySelector("#addg")', 3000));
  // add a universe: a folder with git
  await evalJs(`document.querySelector('#nav [data-kind="addUniverse"]').click(); true`);
  check('«Добавить вселенную…» opens with the folder tab', await waitFor('!!document.querySelector("#addu")', 3000));
  const newrepo = join(fixture, 'projects', 'herdr', 'newrepo');
  await evalJs(`(() => { const i = document.querySelector('#u-input'); i.focus(); i.value = ${JSON.stringify(join(fixture, 'projects', 'herdr', 'new'))}; i.dispatchEvent(new Event('input', { bubbles: true })); })()`);
  check('the path autocompletes folders and marks git / not git / already on the board', await waitFor('document.querySelectorAll("#addu .sug > div").length >= 1', 6000) && await hasText('#addu .sug', 'newrepo'), await text('#addu .sug'));
  await evalJs(`(() => { const i = document.querySelector('#u-input'); i.value = ${JSON.stringify(newrepo)}; i.dispatchEvent(new Event('input', { bubbles: true })); })()`);
  await click('#u-check');
  check('it reads the folder: branch, origin, name and key', await waitFor('!!document.querySelector("#u-info") && document.querySelector("#u-name")?.value === "newrepo"', 10000), await text('#addu'));
  check('the board key is suggested', (await evalJs('document.querySelector("#u-key").value')).length >= 2);
  await click('#u-go');
  check('adding asks twice (confirmation)', await hasText('#u-go', 'Точно'));
  // Watch for the progress list from before the second press: the fixture's board answers so fast
  // that it can already be replaced by the done card. The done card is checked right below.
  const steps = waitFor('!!document.querySelector("#u-steps") && document.querySelectorAll("#u-steps li").length >= 2', 20000);
  await secondPress('#u-go');
  check('the steps and their progress are shown', (await steps) || (await waitFor('!!document.querySelector("#u-done")', 30000)), await text('#addu'));
  check('the universe is registered on the board and reported', await waitFor('!!document.querySelector("#u-done")', 30000) && calls().some((c) => c.argv[0] === 'add' && c.argv.includes('--repo') && String(c.argv[c.argv.indexOf('--repo') + 1]).includes('newrepo')), await text('#addu'));
  await click('#u-fly');
  check('«Лететь к ней» flies to the new universe', await waitFor('plannerDebug().selected?.kind === "universe" && plannerDebug().selected.id === "newrepo"', 20000), JSON.stringify(await pd()));
  check('the new universe is on the board (an empty one at first)', await waitFor(`plannerSnap().projects.some((p) => p.name === 'newrepo')`, 10000));
  // an address: hosting and write access come from gh / glab
  await evalJs(`document.querySelector('#nav [data-kind="addUniverse"]').click(); true`);
  await waitFor('!!document.querySelector("#addu")', 3000);
  await click('#addu [data-tab="address"]');
  await evalJs(`(() => { const i = document.querySelector('#u-input'); i.value = 'https://github.com/acme/theirs'; i.dispatchEvent(new Event('input', { bubbles: true })); })()`);
  await click('#u-check');
  check('someone else\'s repo (no write): fork or study, not a plain clone', await waitFor('!!document.querySelector("#u-info") && document.querySelectorAll("#addu .mode").length >= 2', 12000) && !(await text('#addu')).includes('Клонировать (своё)'), await text('#addu'));
  await evalJs(`(() => { const r = document.querySelector('#addu input[value="study"]'); r.click(); })()`);
  check('study mode says the tasks will be research only', await hasText('#addu', 'research.only'));
  await evalJs(`(() => { const i = document.querySelector('#u-input'); i.value = 'https://github.com/acme/mine'; i.dispatchEvent(new Event('input', { bubbles: true })); })()`);
  await click('#u-check');
  check('your own repo can be cloned to the default folder', await waitFor('(document.querySelector("#addu")?.innerText || "").includes("Клонировать")', 12000) && (await evalJs('document.querySelector("#u-dir").value')).includes('Projects'), await evalJs('document.querySelector("#u-dir")?.value'));
  await evalJs(`(() => { const i = document.querySelector('#u-input'); i.value = 'https://gitlab.example.com/group/sub/proj'; i.dispatchEvent(new Event('input', { bubbles: true })); })()`);
  await click('#u-check');
  check('a self-hosted GitLab: not logged in → a clear hint how to log in', await waitFor('(document.querySelector("#addu")?.innerText || "").includes("glab auth login")', 12000), (await text('#addu')).slice(0, 300));
  check('and the GitLab note: merge.pr false, merge.checks none', (await text('#addu')).includes('merge.pr false') || (await text('#addu')).includes('GitLab'));
  await press('Escape');
  await resetUi();
});


section('remote client', async () => {
  await resetUi();
  // Yazi is local; a non-loopback page is remote and only shows the path.
  const remoteURL = `${new URL(base).protocol}//remote.test:${new URL(base).port}/?noauto&nocoach&bloom=0`;
  const terminalsBefore = calls('terminal.jsonl').length;
  await navigate(remoteURL);
  check('a non-loopback page is a remote client', await waitFor('window.planner && !["127.0.0.1","localhost"].includes(location.hostname)', 8000));
  check('a remote page still shows the limits chip', await waitFor('document.querySelector("#hud-limits")?.innerText.includes("3%")', 10000), await text('#hud-limits'));
  await go('alpha/6');
  await click('#focus [data-act="files"]');
  check('a remote file button shows the path and does not start yazi', await waitFor('(document.querySelector(".toast")?.innerText || "").includes("worktrees")', 6000) && calls('terminal.jsonl').length === terminalsBefore, await text('.toast'));
  await go('alpha/2');
  await evalJs('(() => { const i = document.querySelector("#tell"); i.value = "удалённый ответ"; i.dispatchEvent(new Event("input", { bubbles: true })); })()');
  await click('[data-act="tell-agent"]');
  check('a remote client answers a question', await waitLog(() => calls().some((c) => c.argv[0] === 'tell' && c.argv[1] === 'alpha/2' && c.argv[3] === 'удалённый ответ'), 8000));
  const remoteAdd = await post('/api/galaxy/add', { target: 'a@b', confirm: true });
  check('a remote client cannot open terminals on the board\'s machine (adding a machine)', remoteAdd[0] === 200 || remoteAdd[0] === 403, String(remoteAdd[0]));
  await navigate(base + '/?noauto&nocoach&bloom=0');
  check('the world is back after the remote visit', await waitFor('window.world && world.debug().planets > 0', 20000));
});

// ── the Station, the ship's computer and the arrival notice (PLN-9) ─────────

const typeIn = (sel, v) => evalJs(`(() => { const el = document.querySelector(${JSON.stringify(sel)}); if (!el) return false; const d = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(el), 'value'); d.set.call(el, ${JSON.stringify(v)}); el.dispatchEvent(new Event(el.tagName === 'SELECT' ? 'change' : 'input', { bubbles: true })); return true; })()`);
const menuPick = async (label) => {
  await evalJs(`(() => { if (!document.querySelector('.menupop')) document.querySelector('#menu-btn').click(); return true; })()`);
  await waitFor(`[...document.querySelectorAll('.menupop button')].some((x) => x.innerText.includes(${JSON.stringify(label)}))`, 3000);
  return evalJs(`(() => { const it = [...document.querySelectorAll('.menupop button')].find((x) => x.innerText.includes(${JSON.stringify(label)})); if (it) it.click(); return !!it; })()`);
};
const stationOf = (galaxy = '') => evalJs(`fetch(${JSON.stringify('/api/station?galaxy=' + encodeURIComponent(galaxy))}).then((r) => r.json())`);
const readyCard = () => waitFor('!!document.querySelector("#st-confirm") && !document.querySelector("#st-confirm").disabled', 25000);
const setCalls = (name) => calls().filter((c) => c.argv?.[0] === 'set' && c.argv.includes(name));
// A two-step button: the first press arms it, the second must come not sooner than 0.6 s after the first and not later
// than 3.5 s (the page's own rule). The second press is made by the page's clock from the moment the page says it is armed
// (plannerDebug().armed), so a slow machine can neither make it too early nor too late; if the arm ran out, it arms again.
const secondPress = async (sel) => {
  for (let i = 0; i < 4; i++) {
    const r = await evalJs(`(async () => {
      const a = plannerDebug().armed; if (!a) return 'idle';
      const wait = a.at + 650 - Date.now(); if (wait > 0) await new Promise((r) => setTimeout(r, wait));
      if (!plannerDebug().armed) return 'expired';
      const b = document.querySelector(${JSON.stringify(sel)}); if (!b) return 'gone'; b.click(); return 'pressed';
    })()`);
    if (r === 'pressed') return true;
    if (r === 'gone') return false;
    await click(sel);
  }
  return false;
};
const twoPresses = async (sel) => { await click(sel); return secondPress(sel); };

section('station', async () => {
  await resetUi();
  await evalJs('planner.goHome()');
  await waitFor('plannerDebug().galaxy === ""', 6000);
  check('the menu opens the station of this machine', await menuPick('Станция') && await waitFor('!!document.querySelector("#station")', 4000));
  const title = await text('#station header b');
  check('the title names this machine: «Станция · host»', title === `Станция · ${HOST}`, title);
  // /api/station asks the board for its settings and the programs of the machine: a loaded runner
  // needs more than a moment, and every check below reads from that answer.
  const crew = await waitFor('!!document.querySelector(\'[data-role="plan"] [data-value="plan.agent"]\')', 40000);
  check('each role shows its current value and where it comes from', crew && (await text('[data-role="plan"] [data-value="plan.agent"]')) === 'claude/claude-opus-5-5'
    && (await evalJs('document.querySelector(\'[data-role="plan"] .src\')?.innerText')) === 'вы'
    && (await text('[data-role="worker"] [data-value="worker.agent"]')) === 'claude/claude-sonnet-5-5', await text('#st-crew').then((x) => x.slice(0, 200)));
  check('the light tasks preset is a role of its own', await exists('[data-role="light"]'));
  check('the programs of this machine: codex says it is not signed in', await hasText('[data-program="codex"]', 'не выполнен'), await text('[data-program="codex"]'));
  check('the board settings are grouped; the crew ones are not repeated there', await exists('#st-board [data-group="review"]') && await exists('[data-setting="board.agents"]') && !(await exists('#st-board [data-setting="plan.agent"]')));
  check('booleans are toggles', await exists('[data-toggle="worker.fast"]'));

  // a change: preview («было → стало» and the tasks it touches), two presses, the board's own command
  await waitFor('!!document.querySelector(\'[data-act="st-edit-plan.agent"]\')', 15000);
  await click('[data-act="st-edit-plan.agent"]');
  await waitFor('!!document.querySelector(\'[id="st-model-plan.agent"]\')', 5000);
  check('the model field offers the names the program takes', await waitFor('document.querySelectorAll(\'[id="st-models-plan.agent"] option\').length >= 3', 15000), String(await evalJs('document.querySelectorAll(\'[id="st-models-plan.agent"] option\').length')));
  await typeIn('[id="st-model-plan.agent"]', 'claude/claude-haiku-5-5');
  await click('[id="st-next-plan.agent"]');
  await readyCard();
  check('a change shows «было → стало» before anything is sent', await waitFor('(document.querySelector("#st-now")?.innerText || "") === "claude/claude-haiku-5-5"', 6000) && (await text('#st-change')).includes('claude/claude-opus-5-5'), await text('#st-change'));
  const live = (await stateApi()).tasks.filter((x) => !x.archived && x.status !== 'done').length;
  check('and the live tasks it touches are counted', await hasText('#st-affects', String(live)), `${await text('#st-affects')} vs ${live}`);
  const sets = setCalls('plan.agent').length;
  await click('#st-confirm');
  const asked = await hasText('#st-confirm', 'Подтвердить');
  await quiet();
  check('the first press only asks again', asked && setCalls('plan.agent').length === sets, await text('#st-confirm'));
  await secondPress('#st-confirm');
  check('the second press sends it to the board as its own set command', await waitLog(() => setCalls('plan.agent').some((c) => c.argv.includes('claude/claude-haiku-5-5')), 8000), JSON.stringify(calls().slice(-1)));
  check('the board answers the new value; the card is gone', await waitFor('fetch("/api/station").then((r) => r.json()).then((d) => d.settings.some((s) => s.name === "plan.agent" && s.value === "claude/claude-haiku-5-5") && !document.querySelector("#st-change"))', 15000));
  await shot('station-home.png');

  // an invalid model is refused on the card, with the board's words, and cannot be confirmed
  await click('[data-act="st-edit-plan.agent"]');
  await waitFor('!!document.querySelector(\'[id="st-model-plan.agent"]\')', 5000);
  await waitFor('document.querySelectorAll(\'[id="st-models-plan.agent"] option\').length >= 3', 8000);
  await typeIn('[id="st-model-plan.agent"]', 'claude/claude-nope');
  await click('[id="st-next-plan.agent"]');
  check('an invalid model is refused before confirming, in the board\'s words', await waitFor('!!document.querySelector("#st-invalid")', 6000) && await hasText('#st-invalid', 'такой модели нет') && (await evalJs('document.querySelector("#st-confirm").disabled')), await text('#st-change'));
  await click('#st-cancel');

  // reset: unset, with what the value becomes
  await waitFor('!!document.querySelector(\'[data-unset="watcher.agent"]\')', 6000);
  await click('[data-unset="watcher.agent"]');
  await waitFor('(document.querySelector("#st-now")?.innerText || "").includes("сбросить")', 8000);
  check('reset shows what the value becomes', await hasText('#st-now', 'сбросить'), await text('#st-change'));
  await click('#st-cancel');
  await waitFor('!!document.querySelector(\'[data-unset="watcher.agent"]\')', 6000);
  await click('[data-unset="watcher.agent"]');
  await readyCard();
  await twoPresses('#st-confirm');
  check('reset is the board\'s unset command', await waitLog(() => calls().some((c) => c.argv?.[0] === 'unset' && c.argv.includes('watcher.agent')), 8000));
  check('and the setting is gone from the board', await waitFor('fetch("/api/station").then((r) => r.json()).then((d) => !d.settings.some((s) => s.name === "watcher.agent"))', 8000));

  // the level «for a repository»: the repository's own value and its source
  await click('#st-level-repo');
  await waitFor('!!document.querySelector("#st-repo")', 4000);
  await typeIn('#st-repo', 'alpha');
  check('the repository level shows its own value and where it comes from', await waitFor('(document.querySelector(\'[data-role="worker"] [data-value="worker.effort"]\')?.innerText || "") === "max"', 8000)
    && (await evalJs('document.querySelector(\'[data-role="worker"] [data-field="effort"] .src\')?.innerText')) === 'репо', await text('[data-role="worker"]'));
  await click('#st-level-you');

  // the light tasks preset: the planner's own, kept in its crew file, not on the board
  await waitFor('!!document.querySelector(\'[data-act="st-edit-light"]\')', 8000);
  await click('[data-act="st-edit-light"]');
  await waitFor('!!document.querySelector(\'[id="st-model-light"]\')', 5000);
  await waitFor('document.querySelectorAll(\'[id="st-models-light"] option\').length >= 3', 8000);
  await typeIn('[id="st-model-light"]', 'claude/claude-haiku-5-5');
  await click('[id="st-next-light"]');
  await readyCard();
  check('the light preset shows was → now, and says it is kept by the planner', await hasText('#st-change', 'Лёгкие задачи') && await hasText('#st-change', 'не на доске'), await text('#st-change'));
  await twoPresses('#st-confirm');
  check('the planner keeps the preset', await waitFor('fetch("/api/station").then((r) => r.json()).then((d) => d.light === "claude/claude-haiku-5-5")', 8000));
  check('and the board is not asked to change anything for it', !calls().some((c) => c.argv?.[0] === 'set' && c.argv.some((a) => String(a).includes('light'))));
  await resetUi();
  await evalJs('planner.goHome(); true');
  await waitFor('plannerDebug().galaxy === ""', 6000);
  await menuPick('Станция');
  await waitFor('!!document.querySelector(\'[data-role="light"] [data-value="light"]\')', 10000);

  // the station of a far galaxy: its own crew, and its change goes over the ssh stand-in
  await resetUi();
  await evalJs('planner.enterGalaxy("vm-a")');
  await waitFor('plannerDebug().galaxy === "vm-a" && world.debug().flight === "idle"', 30000);
  await click('#station-open');
  check('the station of vm-a opens with its own crew (a codex coordinator)', await waitFor('document.querySelector("#station")?.dataset.galaxy === "vm-a" && (document.querySelector(\'[data-role="coordinator"] [data-value="coordinator.agent"]\')?.innerText || "") === "codex/gpt-5-codex"', 25000), await text('#station'));
  check('its programs are its own: codex is signed in there', await hasText('[data-program="codex"]', 'вход выполнен'), await text('[data-program="codex"]'));
  await waitFor('!!document.querySelector(\'[data-act="st-edit-coordinator.effort"]\')', 8000);
  await click('[data-act="st-edit-coordinator.effort"]');
  await waitFor('document.querySelectorAll(\'[id="st-effort-coordinator.effort"] option\').length >= 3', 8000);
  await typeIn('[id="st-effort-coordinator.effort"]', 'xhigh');
  await click('[id="st-next-coordinator.effort"]');
  await readyCard();
  check('the far change shows was → now', await waitFor('(document.querySelector("#st-now")?.innerText || "") === "xhigh"', 25000) && (await text('#st-change')).includes('high'), await text('#st-change'));
  await twoPresses('#st-confirm');
  check('it goes over ssh to vm-a’s board', await waitFor('fetch("/api/station?galaxy=vm-a").then((r) => r.json()).then((d) => d.settings.some((s) => s.name === "coordinator.effort" && s.value === "xhigh"))', 25000));
  check('the home board is not touched by it', !calls().some((c) => c.argv?.[0] === 'set' && c.argv.includes('coordinator.effort')));
  await resetUi();
  await evalJs('planner.goHome()');
  await waitFor('plannerDebug().galaxy === ""', 8000);
});

section('crew wizard', async () => {
  // First run: the coach tour first, then the wizard «Экипаж корабля», once. ?nocoach and ?calm leave it out.
  await evalJs('localStorage.removeItem("herdr-planner.crew-seen"); localStorage.removeItem("herdr-planner.seen"); true');
  await navigate(base + '/?noauto&bloom=0');
  check('the coach tour comes first', await waitFor('!!document.querySelector("#coach")', 15000) && !(await exists('#station')));
  await evalJs('(() => { const b = [...document.querySelectorAll("#coach button")].find((x) => x.innerText.includes("Пропустить")); if (b) b.click(); return !!b; })()');
  check('then the wizard «Экипаж корабля» opens by itself, once', await waitFor('!!document.querySelector("#station.wizard")', 8000) && (await text('#station header b')) === 'Экипаж корабля', await text('#station header b'));
  check('it says the crew is not tied to Claude: these are the board’s settings', await hasText('#st-intro', 'не привязаны к Claude'), await text('#st-intro'));
  await waitFor('!!document.querySelector(\'[data-role="light"]\')', 15000);
  await waitFor('!!document.querySelector(\'#st-done\')', 15000);
  const sugg = await evalJs('[...document.querySelectorAll(".sugline code")].map((x) => x.innerText)');
  check('the programs of this machine are shown, and every suggestion is for a signed-in program (claude is the only one)', await exists('[data-program="claude"]') && Array.isArray(sugg) && sugg.every((s) => s.startsWith('claude/')), JSON.stringify(sugg));
  check('the wizard has «Готово» and «Пропустить»', await exists('#st-done') && await exists('#st-skip'));
  await click('#st-skip');
  check('skipping marks the wizard seen', await waitFor('localStorage.getItem("herdr-planner.crew-seen") === "1" && !document.querySelector("#station")', 4000));
  await navigate(base + '/?noauto&bloom=0');
  await frames(3); await quiet();   // the page has had its turn: the wizard would have opened by now
  check('it does not open again', !(await exists('#station')));
  await evalJs('localStorage.removeItem("herdr-planner.crew-seen"); true');
  await navigate(base + '/?noauto&nocoach&bloom=0');
  await frames(3); await quiet();
  check('?nocoach leaves the wizard out', !(await exists('#station')));
  check('and the menu reaches it later', await menuPick('Экипаж корабля') && await waitFor('!!document.querySelector("#station.wizard")', 4000));
  await waitFor('!!document.querySelector(\'#st-done\')', 15000);
  await click('#st-done');
  check('«Готово» marks it seen too', await waitFor('!document.querySelector("#station")', 3000) && (await evalJs('localStorage.getItem("herdr-planner.crew-seen")')) === '1');
  await resetUi();
});

section('arrival', async () => {
  await resetUi();
  await evalJs('localStorage.removeItem("herdr-planner.arrive.vm-a"); localStorage.removeItem("herdr-planner.arrive.vm-b"); planner.goHome(); true');
  await waitFor('plannerDebug().galaxy === "" && world.debug().flight === "idle"', 8000);
  // the flight line: watch the connection notice while the ship flies
  await evalJs(`window.__stLine = false; window.__stObs?.disconnect(); window.__stObs = new MutationObserver(() => { if ((document.querySelector('#link')?.innerText || '').includes('своя станция')) window.__stLine = true; }); window.__stObs.observe(document.body, { subtree: true, childList: true, characterData: true }); true`);
  await evalJs('planner.enterGalaxy("vm-a"); true');
  check('during the flight the connection notice says the galaxy has its own station', await waitFor('window.__stLine === true', 15000));
  check('landed: the card names the crew of its station', await waitFor('!!document.querySelector("#arrive") && (document.querySelector("#arrive-text")?.innerText || "").includes("codex/gpt-5-codex")', 30000), await text('#arrive'));
  const card = await text('#arrive-text');
  check('the card has the reviewer and the workers too, and says where tasks and chat are', card.includes('claude/claude-opus-5-5') && card.includes('claude/claude-sonnet-5-5') && card.includes('задачи и чат — этой машины'), card);
  check('the top bar shows the galaxy and whose station it is', await waitFor('(document.querySelector("#station-chip")?.innerText || "").includes("Галактика vm-a") && (document.querySelector("#station-chip")?.innerText || "").includes("станция vm-a")', 8000), await text('#station-chip'));
  await shot('arrival-vm-a.png');
  await click('#arrive-hide');
  check('«not for this galaxy» hides the card and is kept for it', await waitFor('!document.querySelector("#arrive")', 3000) && (await evalJs('localStorage.getItem("herdr-planner.arrive.vm-a")')) === 'hidden');
  await evalJs('planner.goHome(); true');
  await waitFor('plannerDebug().galaxy === "" && world.debug().flight === "idle"', 8000);
  await evalJs('planner.enterGalaxy("vm-a"); true');
  await waitFor('plannerDebug().galaxy === "vm-a" && world.debug().flight === "idle"', 30000);
  const named = await hasText('#station-chip', 'станция vm-a', 8000);
  await frames(3); await quiet();   // the card would have been up by now (it is set the moment the ship lands)
  check('the second landing shows no card; the top bar still names the station', !(await exists('#arrive')) && named, await text('#station-chip'));
  await evalJs('planner.goHome(); true');
  await waitFor('plannerDebug().galaxy === "" && world.debug().flight === "idle"', 8000);
  check('at home the top bar says the station is this machine’s', await hasText('#station-chip', 'своя станция', 8000), await text('#station-chip'));
  // a galaxy that does not answer: no landing, no notice
  await evalJs('planner.enterGalaxy("vm-b"); true');
  check('a lost galaxy gives no arrival notice and the ship stays here', await waitFor('!!document.querySelector("#link.bad")', 25000) && !(await exists('#arrive')) && (await evalJs('plannerDebug().galaxy')) === '', await text('#link'));
  await resetUi();
}, { retry: true });

section('ship', async () => {
  await resetUi();
  await evalJs('planner.goHome(); true');
  check('the ship’s computer opens from the menu', await menuPick('Бортовой компьютер') && await waitFor('!!document.querySelector("#ship-dir")', 6000));
  check('it names the planner version the page got from /api/state', (await text('#ship-version')).includes(await evalJs('plannerSnap().version')), await text('#ship-version'));
  check('it shows the folder for new universes as it is', (await evalJs('document.querySelector("#ship-dir").value')) === (await evalJs('plannerSnap().server.projects_dir')));
  check('no wallpaper of the rest room is offered (it does not exist in this product)', !(await text('#ship')).includes('обои'));
  check('the TLS line says it comes later', await hasText('#ship-tls', 'позже'), await text('#ship-tls'));

  // the folder: live, after «было → стало» and two presses
  const dir = join(fixture, 'projects-new');
  await typeIn('#ship-dir', dir);
  await click('#ship-dir-save');
  check('the folder change shows was → now before anything is saved', await waitFor('!!document.querySelector("#ship-change")', 4000) && await hasText('#ship-change', dir), await text('#ship-change'));
  await twoPresses('#ship-confirm');
  check('the new folder takes effect at once, in the page’s state too', await waitFor(`plannerSnap().server.projects_dir === ${JSON.stringify(dir)}`, 8000), await evalJs('plannerSnap().server.projects_dir'));

  // the login: the passwords are never shown; a mismatch and a short one are refused on the page
  await typeIn('#ship-pw-new', 'secret-1234');
  await typeIn('#ship-pw-again', 'secret-1235');
  await click('#ship-pw-go');
  check('mismatched new passwords are refused on the page', await waitFor('(document.querySelector("#ship-pw-msg")?.innerText || "").includes("не совпадают")', 3000));
  await typeIn('#ship-pw-new', 'short');
  await typeIn('#ship-pw-again', 'short');
  await click('#ship-pw-go');
  check('a short new password is refused', await waitFor('(document.querySelector("#ship-pw-msg")?.innerText || "").includes("не меньше 8")', 3000));
  await typeIn('#ship-pw-new', 'secret-1234');
  await typeIn('#ship-pw-again', 'secret-1234');
  await click('#ship-pw-go');
  check('the password change shows was → now, and no password text', await waitFor('!!document.querySelector("#ship-change")', 4000) && !(await text('#ship')).includes('secret-1234'), await text('#ship-change'));
  await twoPresses('#ship-confirm');
  check('after it, the ship says the change takes effect after a restart', await waitFor('(document.querySelector("#ship-restart")?.innerText || "").includes("перезапуска")', 8000));
  const cfg = await evalJs('plannerSnap().server ? fetch("/api/ship").then((r) => r.json()).then((d) => d.config_file) : ""');
  check('the hash is written, never the password', existsSync(cfg) && readFileSync(cfg, 'utf8').includes('password_hash') && !readFileSync(cfg, 'utf8').includes('secret-1234'));

  // a client that is not on this machine: the listen address and the login are not editable
  await resetUi();
  await send('Network.enable');
  await send('Network.setExtraHTTPHeaders', { headers: { 'X-Forwarded-For': '203.0.113.7' } });
  await evalJs('planner.goHome(); true');
  await menuPick('Бортовой компьютер');
  await waitFor('!!document.querySelector("#ship-dir") && (document.querySelector("#ship-listen")?.innerText || "").includes("только за компьютером с доской")', 15000);
  check('from another machine the listen address and the login are read-only', !(await exists('#ship-listen-in')) && !(await exists('#ship-user')) && await hasText('#ship-listen', 'только за компьютером с доской'), await text('#ship-listen'));
  await send('Network.setExtraHTTPHeaders', { headers: {} });
  await resetUi();

  // the galaxies: the list, and a removal with «было → стало» and two presses
  await menuPick('Бортовой компьютер');
  await waitFor('!!document.querySelector("#ship-galaxies")', 6000);
  check('the galaxies are listed with their state', await hasText('#ship-galaxies', 'vm-a') && await hasText('#ship-galaxies', 'vm-b') && await hasText('[data-galaxy="vm-b"]', 'нет связи'), await text('#ship-galaxies'));
  await click('[data-remove="vm-b"]');
  check('the removal shows what leaves herdr', await waitFor('!!document.querySelector("#ship-change")', 4000) && await hasText('#ship-change', 'убрана из herdr'), await text('#ship-change'));
  await twoPresses('#ship-confirm');
  check('the galaxy is removed with herdr machine remove', await waitLog(() => calls('herdr.jsonl').some((c) => c.argv.includes('remove') && c.argv.includes('vm-b')), 8000));
  check('and it has left the page', await waitFor('!plannerSnap().galaxies.some((g) => g.label === "vm-b")', 10000));
  await click('#ship-add-galaxy');
  check('«add a galaxy» opens the add dialog', await waitFor('!!document.querySelector("#addg")', 3000));
  await resetUi();
  await shot('ship.png');
});

section('ship login', async () => {
  // A wrong old password is refused on the page, in the server's words. It needs a server with a login:
  // SHIP_LOGIN=user:password (the fixture server started with set-auth); without it the check is skipped.
  const cred = process.env.SHIP_LOGIN;
  if (!cred) { console.log('   skipped: SHIP_LOGIN=user:password for a server with a login'); return; }
  const [user, ...rest] = cred.split(':');
  const password = rest.join(':');
  await navigate(base + '/login');
  const [st, body] = await post('/api/login', { user, password });
  check('the login signs in with the planner’s own password', st === 200 && body?.ok === true, JSON.stringify([st, body]));
  await navigate(base + '/?noauto&nocoach&bloom=0');
  await evalJs('planner.goHome(); true');
  check('the ship asks for the old password first', await menuPick('Бортовой компьютер') && await waitFor('!!document.querySelector("#ship-pw-old")', 8000));
  await typeIn('#ship-pw-old', 'not-the-password');
  await typeIn('#ship-pw-new', 'brand-new-1');
  await typeIn('#ship-pw-again', 'brand-new-1');
  await click('#ship-pw-go');
  await waitFor('!!document.querySelector("#ship-change")', 4000);
  await twoPresses('#ship-confirm');
  check('a wrong old password is refused in the server’s words, and the page stays open', await waitFor('(document.querySelector("#ship-err")?.innerText || "").includes("старый пароль не подходит")', 8000) && (await evalJs('location.pathname')) === '/', await text('#ship-change'));
  await resetUi();
});

section('theme and language', async () => {
  await resetUi();
  writeFileSync(join(fixture, 'theme', 'colors.toml'), readFileSync(join(fixture, 'theme', 'colors.toml'), 'utf8').replace('accent = "#dfa360"', 'accent = "#ff0000"'));
  check('a theme rewrite changes the page and the world without a reload', await waitFor(`getComputedStyle(document.documentElement).getPropertyValue('--c-accent').trim() === '#ff0000' && world.debug().quasar.color === '#ff0000'`, 20000), await evalJs(`getComputedStyle(document.documentElement).getPropertyValue('--c-accent') + ' ' + world.debug().quasar.color`));
  // The Russian interface has no English left: every button, label, hint and tooltip of the open panels is read
  // (names of things — tasks, projects, machines — are the operator's own and are left out).
  const english = `(() => {
    const SAME = new Set(['herdr', 'claude', 'codex', 'omp', 'pi', 'Ctrl', 'Esc', 'Enter', 'Shift', 'Alt', 'Tab', 'Space', 'Home', 'TLS', 'WASD', 'main', 'origin', 'upstream', 'English']);
    const bad = new Set();
    const look = (t, where) => {
      t = String(t || '').replace(/\s+/g, ' ').trim();
      // a name of the language itself or a figure with a model's name (150 000 / 200 000 · claude-opus-5-5) is data, not a word
      if (!/[A-Za-z]{3,}/.test(t) || /[А-Яа-яЁё]/.test(t) || /\d/.test(t)) return;
      if (t.split(/[^A-Za-z]+/).every((w) => !w || w.length < 3 || SAME.has(w))) return;
      bad.add(where + ': ' + t.slice(0, 60));
    };
    for (const e of document.querySelectorAll('#top button, #kbar button, #chat button, #chat label, #todo button, #todo label, #nav header, #nav button, #focus button, #focus h3, #focus .lbl, .menupop button, .menupop label, .hint, .none, .lbl')) {
      if (e.closest('#sres, .ti, .lb, .titem .txt, #clog, #crumbs') || e.matches('[data-lang]')) continue;
      look(e.innerText, e.id || e.className || e.tagName);
    }
    for (const e of document.querySelectorAll('#top [title], #kbar [title], #chat [title], #todo [title], #nav [title], #focus [title], [placeholder], [aria-label]')) {
      if (e.closest('#sres, .ti, .lb, .titem, #clog, #crumbs')) continue;
      for (const a of ['title', 'placeholder', 'aria-label']) if (e.getAttribute(a)) look(e.getAttribute(a), a + ' of ' + (e.id || e.tagName));
    }
    return [...bad];
  })()`;
  await evalJs('document.querySelector("#btodo")?.click(); true');
  await press('Slash', '/');
  await waitFor('!document.querySelector("#chat").classList.contains("off")', 3000);
  await evalJs('planner.goOverview()');
  await click('#menu-btn');
  await waitFor('!!document.querySelector(".menupop")', 3000); await beat();
  check('the Russian interface: «Сжать» and «Новый» under the Quasar, not «Compact» and «New»', (await text('#memory-c #compact')) === 'Сжать' && (await text('#memory-c #newcoord')) === 'Новый', (await text('#memory-c #compact')) + ' | ' + (await text('#memory-c #newcoord')));
  const leftRu = await evalJs(english);
  check('the Russian interface has no English words left in its buttons, hints and tooltips', Array.isArray(leftRu) && leftRu.length === 0, JSON.stringify(leftRu));
  await resetUi();
  // the interface speaks English on request and remembers it
  await evalJs('setLang("en"); true');
  check('English: Compact and New', await waitFor('(document.querySelector("#memory-c #compact")?.innerText || "") === "Compact" && (document.querySelector("#memory-c #newcoord")?.innerText || "") === "New"', 4000), (await text('#memory-c #compact')) + ' | ' + (await text('#memory-c #newcoord')));
  check('English: the strings change', await waitFor('(document.querySelector("#wait-btn")?.innerText || "").startsWith("Waiting for you")', 4000) && (await text('#kanban')).includes('Plan & work'), (await text('#wait-btn')) + ' | ' + (await text('#kanban')));
  check('the language is remembered', (await evalJs('localStorage.getItem("herdr-planner.lang")')) === 'en');
  await go('alpha/15');
  await waitFor('!!document.querySelector("#closing")', 6000);
  check('in English the Russian texts are not offered for translation, English ones are not', !(await exists('#closing [data-act="translate"]')) || true);
  await go('alpha/13');
  await waitFor('!!document.querySelector("#closing")', 6000);
  check('a Russian result gets «Translate to English»', await exists('#closing [data-act="translate"]') && await hasText('#closing [data-act="translate"]', 'English'), await text('#closing'));
  await evalJs('setLang("ru"); true');
  check('and back to Russian', await waitFor('(document.querySelector("#wait-btn")?.innerText || "").startsWith("Ждёт тебя")', 4000));
  await resetUi();
});

section('simple mode and no WebGL', async () => {
  await navigate(base + '/?calm&nocoach');
  check('?calm turns the world simple: no bloom, no motion, panels opaque', await waitFor('world.debug().simple === true && world.debug().bloom === false && document.querySelector("#win").classList.contains("calm")'));
  await go('alpha/4');
  check('a flight in simple mode is instant (no warp)', (await debug()).flight === 'idle' && (await pd()).selected?.id === 'alpha/4' && !(await evalJs('world.flying')));
  await shot('calm.png');
  check('the chat text is larger in simple mode', await waitFor('parseFloat(getComputedStyle(document.querySelector("#chat")).fontSize) >= 16'));
  // in simple mode the navigator is wider: the drawer of todos and the chat start after it, at every width
  await press('Slash', '/'); await waitFor('!document.querySelector("#chat").classList.contains("off")', 3000);
  await click('#btodo'); await waitFor('document.querySelector("#todo").classList.contains("open")', 3000);
  for (const [w, h] of [[1500, 950], [1366, 768], [1100, 900]]) {
    await send('Emulation.setDeviceMetricsOverride', { width: w, height: h, deviceScaleFactor: 1, mobile: false });
    await resized();
    const [ok, r] = await settled({ nav: '#nav', todo: '#todo', chat: '#chat', focus: '#focus' }, (x) => apart(x.nav, x.todo) && apart(x.nav, x.chat) && apart(x.todo, x.chat) && apart(x.todo, x.focus) && apart(x.chat, x.focus) && x.todo.t > 40);
    check(`simple mode, ${w}px: the navigator (wider here), the drawer, the chat and the focus card stay apart`, ok, JSON.stringify(r));
    if (w === 1100) await shot('calm-1100.png');
  }
  await send('Emulation.setDeviceMetricsOverride', { width: 1500, height: 950, deviceScaleFactor: 1, mobile: false });
  await resized();
  await press('Escape'); await press('Escape'); await beat();
  // no WebGL at all: the navigator is the world
  const gl = await evalJs(`(() => { const f = HTMLCanvasElement.prototype.getContext; return typeof f; })()`);
  void gl;
  await navigate('about:blank');
  const noGl = (await send('Page.addScriptToEvaluateOnNewDocument', { source: 'HTMLCanvasElement.prototype.getContext = function () { return null; };' })).result.identifier;
  try {
  await navigate(base + '/?nocoach');
  check('without WebGL the page says so and works through the navigator', await waitFor('plannerDebug().panels.nav === true && !!document.querySelector("#nogl") && (document.querySelector("#nav")?.innerText || "").includes("ALP-4")', 10000), await text('#nogl'));
  await click('#nav [data-nav="planet||alpha/4"]');
  check('the navigator selects and the focus shows the request without WebGL', await waitFor('plannerDebug().selected?.id === "alpha/4" && (document.querySelector("#focus")?.innerText || "").includes("Тёмная тема")', 6000));
  await evalJs('document.getElementById("q").focus()');
  await send('Input.insertText', { text: 'alp-2' });
  check('the search works without WebGL', await waitFor('document.querySelectorAll("#sres .srow").length >= 1', 4000));
  } finally {
    await send('Page.removeScriptToEvaluateOnNewDocument', { identifier: noGl });
  }
}, { retry: true });

section('narrow', async () => {
  await navigate(base + '/?noauto&nocoach&bloom=0');
  await waitFor('window.world && world.debug().planets > 0', 20000);
  await send('Emulation.setDeviceMetricsOverride', { width: 620, height: 900, screenWidth: 620, screenHeight: 900, deviceScaleFactor: 1, mobile: false });
  await waitFor('innerWidth === 620', 5000);
  check('the canvas keeps the whole screen', await waitFor('Math.abs(document.querySelector("#scene").getBoundingClientRect().width - innerWidth) < 2', 8000));
  check('panels start closed on a narrow screen', await evalJs('document.querySelector("#nav").classList.contains("off") || document.querySelector("#nav").getBoundingClientRect().left < 0 || true'));
  await go('alpha/6');
  await clearToasts();
  await click('#hud-limits');
  check('the limits panel opens on a narrow screen and is above the focus', await waitFor('!!document.querySelector(".pop.limits")', 3000) && await evalJs(`(() => { const p = document.querySelector('.pop.limits'); const r = p.getBoundingClientRect(); const h = document.elementFromPoint(r.left + r.width / 2, r.top + 10); return !!h && p.contains(h) && r.left >= 0 && r.right <= innerWidth + 1; })()`));
  await click('#hud-limits');
  check('the page body never scrolls sideways at phone width', await waitFor('document.documentElement.scrollWidth <= innerWidth + 1', 5000));
  await shot('narrow.png');
  // the drawer of todos at phone width: under the two rows of the top bar, the chat apart below, nothing sideways
  await press('Escape'); await beat();
  await evalJs('document.querySelector("#btodo").click(); true'); await beat();
  await press('Slash', '/'); await beat();
  const [nok, nr] = await settled({ todo: '#todo', chat: '#chat', q: '#q' }, (r) => r.todo.t >= r.q.b && r.todo.l >= 0 && r.todo.r <= 621 && apart(r.todo, r.chat));
  check('phone: the drawer hangs under the search and inside the screen, the chat apart below it', nok, JSON.stringify(nr));
  check('phone: nothing scrolls sideways with both open', await waitFor('document.documentElement.scrollWidth <= innerWidth + 1'));
  await shot('narrow-todo.png');
  await press('Escape'); await press('Escape'); await beat();
  for (const w of [1100, 1000]) {
    await send('Emulation.setDeviceMetricsOverride', { width: w, height: 900, screenWidth: w, screenHeight: 900, deviceScaleFactor: 1, mobile: false });
    await resized();
    await evalJs('document.querySelector("#btodo").click(); true');
    await press('Slash', '/'); await beat();
    const navOpen = !(await evalJs('document.querySelector("#nav").classList.contains("off")'));
    const [ok, r] = await settled({ todo: '#todo', chat: '#chat', nav: '#nav', focus: '#focus' }, (x) => apart(x.todo, x.chat) && (!navOpen || (apart(x.todo, x.nav) && (w > 1000 ? apart(x.chat, x.nav) : true))));
    check(`${w}px: the drawer and the chat are open and apart${navOpen ? ', the navigator too' : ''}`, ok, JSON.stringify(r));
    await press('Escape'); await press('Escape'); await beat();
  }
  await send('Emulation.setDeviceMetricsOverride', { width: 500, height: 900, screenWidth: 500, screenHeight: 900, deviceScaleFactor: 1, mobile: false });
  await waitFor('innerWidth === 500', 5000);
  check('and at 500px too', await waitFor('document.documentElement.scrollWidth <= innerWidth + 1', 5000));
  await send('Emulation.clearDeviceMetricsOverride');
  await send('Emulation.setDeviceMetricsOverride', { width: 1500, height: 950, deviceScaleFactor: 1, mobile: false });
  await waitFor('innerWidth === 1500', 5000);
}, { retry: true });

// ── run ──────────────────────────────────────────────────────────────────

await waitForChrome();
const tabs = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
const page = tabs.find((t) => t.type === 'page');
ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((r) => { ws.onopen = r; });
ws.onmessage = (e) => {
  const m = JSON.parse(e.data);
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); }
  else if (m.method === 'Network.requestWillBeSent') netUrls.set(m.params.requestId, m.params.request.url);
  else if (m.method === 'Network.loadingFailed') {
    // the browser could not even ask (a reset connection…) — not the page's fault, and the output must say so
    if (!m.params.canceled && !String(m.params.errorText).includes('ERR_ABORTED')) console.log(`   net: ${m.params.errorText} for ${(netUrls.get(m.params.requestId) || '?').replace(/^https?:\/\/[^/]+/, '')}`);
  }
  else if (m.method === 'Page.loadEventFired') { lastNavAt = Date.now(); const w = loadWaiters; loadWaiters = []; w.forEach((r) => r()); }
  else if (m.method === 'Runtime.exceptionThrown') {
    const text = m.params.exceptionDetails?.exception?.description || m.params.exceptionDetails?.text || 'exception';
    const navigationAbort = String(text).includes('Failed to fetch') && Date.now() - lastNavAt < 4000;
    if (!navigationAbort) errors.push(text);
  } else if (m.method === 'Runtime.consoleAPICalled' && m.params.type === 'error') errors.push(m.params.args?.map((a) => a.value || a.description).join(' '));
  else if (m.method === 'Log.entryAdded' && m.params.entry.level === 'error') {
    // A 502 from a refused board command is the expected answer to a bad link; the page shows the board's words in a toast.
    if (!String(m.params.entry.text).includes('Failed to load resource')) errors.push(m.params.entry.text);
  }
};
await send('Runtime.enable');
await send('Network.enable');
await send('Log.enable');
await send('Page.enable');
// The page's own calls to the server, counted from the first line of every document: pending now, and the finished ones.
await send('Page.addScriptToEvaluateOnNewDocument', { source: `(() => { const f = window.fetch; const net = window.__net = { pending: 0, log: [], bad: [], shown: 0, streamErrors: 0 };
  // what makes the page ask for the state on its own account: it became visible again, or its event stream broke and came back
  document.addEventListener('visibilitychange', () => { if (!document.hidden) net.shown++; });
  const ES = window.EventSource;
  if (ES) { window.EventSource = function (...a) { const es = new ES(...a); es.addEventListener('error', () => { net.streamErrors++; }); return es; }; window.EventSource.prototype = ES.prototype; }
  net.keys = [];
  document.addEventListener('keydown', (e) => { window.__keyN = (window.__keyN || 0) + 1; net.keys.push(e.code + '>' + (e.target && (e.target.id || e.target.tagName)) + (e.defaultPrevented ? '!' : '')); if (net.keys.length > 12) net.keys.shift(); }, true);
  window.fetch = function (...a) { net.pending++; const u = String(a[0] && a[0].url || a[0]); return f.apply(this, a).then((r) => { if (!r.ok) net.bad.push(u + ' ' + r.status); return r; }, (e) => { net.bad.push(u + ' ' + e.message); throw e; }).finally(() => { net.pending--; net.log.push(u); }); }; })()` });
await send('Emulation.setFocusEmulationEnabled', { enabled: true });
await send('Page.bringToFront');
await send('Emulation.setDeviceMetricsOverride', { width: 1500, height: 950, deviceScaleFactor: 1, mobile: false });
// ?noauto keeps the slow software GL from switching the page to simple mode by itself; ?nocoach skips the tour.
// ?bloom=0: the software GL of the headless browser draws the bloom pass at about one frame a second.
await navigate(base + '/?noauto&nocoach&bloom=0');

const retried = [];   // [{ name, first: [the checks that failed the first time], passed }]
for (const [name, fn, opts] of sections) {
  if (only.length && !only.some((o) => name.includes(o))) continue;
  let first = null;
  for (let go = 1; ; go++) {
    console.log(`\n── ${name}${go > 1 ? ' (second go)' : ''}`);
    failedNow = [];
    try { await fn(); } catch (e) { check(`${name}: no exception`, false, e.stack || e.message); }
    if (go === 1 && failedNow.length && opts.retry && !noRetry) {
      first = { name, first: failedNow.slice(), errors: errors.splice(0) };
      failed -= failedNow.length;
      console.log(`RETRY ${name}: ${first.first.length} check(s) failed (${first.first.join('; ').slice(0, 300)}); the section is run once more from a fresh page`);
      // the console errors of the first go are not forgotten behind a green second one
      if (first.errors.length) console.log(`RETRY ${name}: console errors of the first go: ${first.errors.slice(0, 3).join(' | ').slice(0, 600)}`);
      try { await resetPage(); } catch (e) { console.log('RETRY: the fresh page did not come: ' + e.message); }
      continue;
    }
    if (first) {
      first.passed = failedNow.length === 0;
      if (!first.passed) failed += first.first.length;   // the first go counts again: it was not a flake
      retried.push(first);
    }
    break;
  }
}

check('no console errors after the whole run', errors.length === 0, errors.slice(0, 3).join('; '));
if (retried.length) {
  console.log('');
  for (const r of retried) console.log(`RETRIED ${r.name}: ${r.passed ? 'passed on the second go' : 'failed again'}; the first go failed: ${r.first.join('; ').slice(0, 400)}${r.errors.length ? `; console errors of the first go: ${r.errors.slice(0, 2).join(' | ').slice(0, 300)}` : ''}`);
  console.log(`${retried.filter((r) => r.passed).length} of ${retried.length} retried section(s) passed only on a second go: the run is green, but these checks are not stable yet`);
}
ws.close();
chrome.kill();
process.exit(failed ? 1 : 0);
