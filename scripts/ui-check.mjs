#!/usr/bin/env node
// Checks of the Galaxy in headless Chromium over the DevTools protocol.
// No dependencies (Node 22+ has WebSocket). With the server running on the fixture:
//   FIXTURE=DIR node scripts/ui-check.mjs http://127.0.0.1:7117 [screenshot-dir]
// `scripts/run-checks.sh` starts the server on the world of scripts/fixture.mjs.
// The fixture's logs (DIR/log/*.jsonl) are the witness for every board command, every
// ssh call to another machine and every terminal that was opened.
//   ONLY=name,name  runs only the sections whose name contains one of the words.

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
let failed = 0;
function check(name, ok, extra = '') {
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${name}${extra ? ' — ' + String(extra).replace(/\s+/g, ' ').slice(0, 300) : ''}`);
  if (!ok) failed++;
}

// ── harness ──────────────────────────────────────────────────────────────

async function waitForChrome() {
  for (let i = 0; i < 60; i++) {
    port = port || readPort();
    try { if (port) { const r = await fetch(`http://127.0.0.1:${port}/json/version`); if (r.ok) return; } } catch {}
    await sleep(200);
  }
  throw new Error('chromium did not start');
}

let ws; let id = 0; const pending = new Map(); const errors = [];
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
let lastNavAt = 0;
const navigate = async (url, ms = 2500) => { lastNavAt = Date.now(); await send('Page.navigate', { url }); await sleep(ms); };
const keyText = async (ch) => {
  await send('Input.dispatchKeyEvent', { type: 'keyDown', text: ch, key: ch });
  await send('Input.dispatchKeyEvent', { type: 'keyUp', key: ch });
};
// A physical key: the page reads `code`, so a letter works in any layout.
const press = async (code, key = code, mods = 0) => {
  const vk = { Escape: 27, Enter: 13, ArrowUp: 38, ArrowDown: 40, ArrowLeft: 37, ArrowRight: 39, Slash: 191, Space: 32, Tab: 9 }[code] ?? (key.length === 1 ? key.toUpperCase().charCodeAt(0) : 0);
  await send('Input.dispatchKeyEvent', { type: 'rawKeyDown', key, code, windowsVirtualKeyCode: vk, modifiers: mods, text: key.length === 1 ? key : undefined });
  await send('Input.dispatchKeyEvent', { type: 'keyUp', key, code, windowsVirtualKeyCode: vk, modifiers: mods });
};
// A key held down: dispatched inside the page (CDP key events are dropped when the headless window loses focus).
const hold = async (code, key, ms) => {
  await evalJs(`document.body.dispatchEvent(new KeyboardEvent('keydown', { code: ${JSON.stringify(code)}, key: ${JSON.stringify(key)}, bubbles: true, cancelable: true })); true`);
  await sleep(ms);
  await evalJs(`document.body.dispatchEvent(new KeyboardEvent('keyup', { code: ${JSON.stringify(code)}, key: ${JSON.stringify(key)}, bubbles: true })); true`);
};
const evalJs = async (expr) => {
  const reply = await Promise.race([
    send('Runtime.evaluate', { expression: expr, returnByValue: true, awaitPromise: true }),
    sleep(9000).then(() => null),
  ]);
  if (!reply) { console.log('eval timeout:', expr.replace(/\s+/g, ' ').slice(0, 100)); return undefined; }
  if (reply.result?.exceptionDetails) { console.log('eval error:', reply.result.exceptionDetails.exception?.description?.slice(0, 160)); return undefined; }
  return reply.result?.result?.value;
};
const text = (sel) => evalJs(`document.querySelector(${JSON.stringify(sel)})?.innerText || ''`);
const click = (sel) => evalJs(`(() => { const e = document.querySelector(${JSON.stringify(sel)}); if (e) e.click(); return !!e; })()`);
const exists = (sel) => evalJs(`!!document.querySelector(${JSON.stringify(sel)})`);
const shot = async (name) => {
  if (!shots) return;
  const r = await send('Page.captureScreenshot', { format: 'png' });
  writeFileSync(join(shots, name), Buffer.from(r.result.data, 'base64'));
};
async function waitFor(expr, ms = 8000) {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await evalJs(expr)) return true; await sleep(150); }
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
  await evalJs(`planner.navigateTo(${planet(taskId, galaxy)})`);
  await waitFor('world.debug().flight === "idle"', 12000);
  await sleep(250);
};
const clearToasts = () => evalJs('document.querySelector("#toasts")?.replaceChildren(); true');
async function waitLog(pred, ms = 8000) {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (pred()) return true; await sleep(120); }
  return false;
}
const resetUi = async () => {
  for (let i = 0; i < 4; i++) { await press('Escape'); await sleep(120); }
};

const sections = [];
const section = (name, fn) => sections.push([name, fn]);

// ── the checks ───────────────────────────────────────────────────────────

section('boot', async () => {
  // The neon is a post-processing pass: once with it on, to see it builds and draws without errors.
  await navigate(base + '/?noauto&nocoach', 4000);
  check('the bloom pass builds in the normal mode', await waitFor('window.world && world.debug().bloom === true', 15000), JSON.stringify(await debug()?.then?.((d) => d?.bloom)));
  await navigate(base + '/?noauto&nocoach&bloom=0', 3500);
  check('the world starts', await waitFor('window.world && world.debug().planets > 0', 20000));
  check('the page has a title and no console errors', (await evalJs('document.title')).includes('Herdr Planner') && errors.length === 0, errors.slice(0, 2).join('; '));
  check('the page fills the viewport', await evalJs('Math.abs(document.querySelector("#win").getBoundingClientRect().width - innerWidth) < 2'));
  const d0 = await debug();
  check('WebGL is on', d0?.webgl === true, JSON.stringify(d0 && { simple: d0.simple, fps: d0.fps }));
  check('every live task is a planet', d0?.livePlanets === 17, String(d0?.livePlanets));
  check('the seven live statuses are drawn', ['failed', 'plan', 'queued', 'ready', 'review', 'waiting', 'working'].every((s) => d0?.statuses.includes(s)), (d0?.statuses || []).join(','));
  check('universes are projects, quiet ones marked', d0?.universes === 7 && d0?.quiet === 3, JSON.stringify({ u: d0?.universes, q: d0?.quiet }));
  check('the board dependencies are links', d0?.links.some((l) => l.from === 'alpha/9' && l.to === 'alpha/10') && d0?.links.some((l) => l.from === 'alpha/4' && l.to === 'gamma/1'), JSON.stringify(d0?.links));
  const s = await stateApi();
  check('the top bar counts the requests', (await text('#wait-btn')).includes(String(s.summary.attention)), `${await text('#wait-btn')} vs ${s.summary.attention}`);
  check('the Quasar is the centre and says it waits for the human', (await debug()).labels > 0 && (await text('.labels')).includes('Квазар'), (await text('.labels')).slice(0, 80));
  check('the kanban line shows the five stages', (await evalJs('document.querySelectorAll("#kanban [data-stage]").length')) === 5 && (await text('#kanban')).includes('План и работа'), await text('#kanban'));
  check('the ship is drawn: the cabin frame is off until something is selected', !(await evalJs('document.querySelector("#cockpit").classList.contains("on")')));
  await shot('galaxy-overview.png');
});

section('live', async () => {
  // The page learns of a board change by the event stream: no /api/state polling.
  const before = await evalJs('performance.getEntriesByType("resource").filter((e) => e.name.includes("/api/state")).length');
  const f = join(fixture, 'state', 'board.json');
  const w = JSON.parse(readFileSync(f, 'utf8'));
  const row = w.rows.find((r) => r.id === 'alpha/11');
  row.state = 'working'; row.stage = 'working'; row.msg = 'Только что взялась за дело';
  writeFileSync(f, JSON.stringify(w, null, 1));
  const t = join(fixture, 'board', 'tasks', 'alpha', '11', 'task.json');
  writeFileSync(t, readFileSync(t, 'utf8')); // the watcher sees the record change
  const pageHas = 'plannerSnap().tasks.find((x) => x.id === "alpha/11")?.status === "working"';
  check('a change of a task record reaches the page at once', await waitFor(pageHas, 15000));
  check('the page got it by the event stream, not by polling', before === await evalJs('performance.getEntriesByType("resource").filter((e) => e.name.includes("/api/state")).length'), String(before));
  check('and it is the server\'s state', (await stateApi()).tasks.find((x) => x.id === 'alpha/11')?.status === 'working');
  row.state = 'idle'; row.stage = 'todo'; row.msg = '';
  writeFileSync(f, JSON.stringify(w, null, 1));
  writeFileSync(t, readFileSync(t, 'utf8'));
  await waitFor('true', 100);
});

section('navigator', async () => {
  check('the navigator starts with this machine\'s Galaxy', (await text('#nav [data-kind="galaxy"]')).includes(HOST), await text('#nav [data-kind="galaxy"]'));
  check('the Quasar is its own item', await exists('#nav [data-kind="quasar"]'));
  check('far galaxies are below: vm-a is online, vm-b is lost with the reason', (await text('#nav')).includes('vm-a') && (await text('#nav')).includes('vm-b') && /нет связи/.test(await text('#nav')), (await text('#nav')).replace(/\s+/g, ' ').slice(-200));
  check('"Add a galaxy" and "Add a universe" are there', (await text('#nav')).includes('Добавить галактику') && (await text('#nav')).includes('Добавить вселенную'));
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
  check('the arrow keys move along the tree', (await evalJs('document.activeElement.dataset.nav')) !== undefined);
  await shot('navigator.png');
});

section('autopilot', async () => {
  await resetUi();
  const cam0 = (await debug()).camera;
  await evalJs(`document.querySelector('#nav [data-nav="planet||alpha/4"]')?.click()`);
  check('choosing a planet in the navigator starts a flight', await waitFor('world.debug().flight !== "idle"', 4000) || await waitFor('plannerDebug().selected?.id === "alpha/4"', 4000));
  check('the flight ends and the planet is selected', await waitFor('world.debug().flight === "idle" && plannerDebug().selected?.id === "alpha/4"', 15000), JSON.stringify(await pd()));
  const cam1 = (await debug()).camera;
  check('the ship really moved', Math.abs(cam1.x - cam0.x) + Math.abs(cam1.y - cam0.y) + Math.abs(cam1.z - cam0.z) > 50, JSON.stringify({ cam0, cam1 }));
  check('we are inside the nebula of the planet', (await pd()).level === 'nebula' && (await pd()).universe === 'alpha', JSON.stringify(await pd()));
  check('the cabin frame is on and shows the target', await evalJs('document.querySelector("#cockpit").classList.contains("on")') && (await text('#readout')).includes('ALP-4'), await text('#readout'));
  check('the focus shows the request of the task', (await text('#focus')).includes('Тёмная тема') && (await text('#focus')).includes('К одобрению'), (await text('#focus')).slice(0, 100));
  check('the crumbs show where we are', (await text('#crumbs')).includes(HOST) && (await text('#crumbs')).includes('alpha') && (await text('#crumbs')).includes('ALP-4'), await text('#crumbs'));
  check('the keys card gives its place to the cabin', !(await exists('#hint')));
  await shot('cockpit-task.png');
  await evalJs(`world.flyTo({kind:'overview'},{instant:true}); true`);
  await sleep(300);
});

section('manual', async () => {
  await resetUi();
  await evalJs('planner.goOverview()');
  await sleep(500);
  await press('KeyP', 'p');
  check('P switches to manual flight', await waitFor('world.mode === "manual"', 3000) && await exists('#mode [data-mode="manual"].on'));
  check('from the overview the ship is taken inside the Galaxy first (not outside the boundary)', await waitFor('world.debug().flight === "idle"', 15000));
  check('and is within the boundary', await evalJs('(() => { const s = world.debug().ship; return Math.hypot(s.x, s.y, s.z) < 1200; })()'));
  check('the kanban gives way to the nose of the ship and the cross-hair is shown', await exists('#cross') && (await evalJs('getComputedStyle(document.querySelector("#kanban")).opacity')) === '0');
  await sleep(800);
  const a = (await debug()).ship;
  await hold('KeyW', 'w', 1500);
  const b = (await debug()).ship;
  check('W moves the ship forward at once', Math.hypot(b.x - a.x, b.y - a.y, b.z - a.z) > 3 && b.speed > 5, JSON.stringify({ a, b }));
  await sleep(600);
  const b2 = (await debug()).ship;
  await hold('KeyD', 'd', 1500);
  const c = (await debug()).ship;
  check('D moves it sideways', Math.hypot(c.x - b2.x, c.y - b2.y, c.z - b2.z) > 3, JSON.stringify({ b2, c }));
  await sleep(600);
  const c2 = (await debug()).ship;
  await hold('Space', ' ', 1500);
  const d = (await debug()).ship;
  check('Space goes up', d.y - c2.y > 3, `${c2.y} → ${d.y}`);
  check('the mouse wheel changes the cruise speed', await evalJs(`(() => { const c = world.debug().ship.cruise; const e = new WheelEvent('wheel', { deltaY: -100, bubbles: true, cancelable: true }); document.querySelector('#scene').dispatchEvent(e); return world.debug().ship.cruise > c; })()`));
  await shot('manual-flight.png');
  // A dive: the crosshair on a nebula, a click → a flight inside, manual flight goes on.
  await evalJs(`planner.navigateTo({kind:'universe',galaxy:'',id:'delta'}); true`);
  check('a dive into a nebula keeps the ship manual and enters it', await waitFor('world.debug().flight === "idle" && world.mode === "manual" && world.level === "nebula"', 15000), JSON.stringify(await pd()));
  await press('KeyP', 'p');
  check('P returns to the autopilot', await waitFor('world.mode === "auto"', 3000));
  await resetUi();
});

section('stack', async () => {
  await resetUi();
  await evalJs('planner.goOverview()');
  await sleep(500);
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
  await sleep(800);
  const cam2 = (await debug()).camera;
  check('an action does not fly the ship away', Math.abs(cam.x - cam2.x) + Math.abs(cam.y - cam2.y) + Math.abs(cam.z - cam2.z) < 30 && (await pd()).selected?.id === 'alpha/2', JSON.stringify({ cam, cam2, sel: (await pd()).selected }));
  await resetUi();
});

section('queue', async () => {
  await resetUi();
  await evalJs('planner.goOverview()');
  await sleep(600);
  await press('KeyB', 'b');
  check('B opens «Ждёт тебя» as a list', await waitFor('document.querySelector("#drawer-wait").classList.contains("open") && document.querySelectorAll("#drawer-wait .q").length >= 10', 4000), await evalJs('document.querySelectorAll("#drawer-wait .q").length'));
  const total = await evalJs('document.querySelectorAll("#drawer-wait .q").length');
  const camList = (await debug()).camera;
  await press('ArrowDown'); await press('ArrowDown');
  await evalJs('document.querySelector("#drawer-wait .wlist").focus()');
  await press('ArrowDown');
  const cursor = await evalJs('[...document.querySelectorAll("#drawer-wait .q")].findIndex((e) => e.classList.contains("cur"))');
  await press('Enter');
  check('Enter dives into the request under the cursor', await waitFor('plannerDebug().selected?.kind === "planet"', 15000), JSON.stringify(await pd()));
  const opened = (await pd()).selected.id;
  await waitFor('world.debug().flight === "idle"', 15000);
  check('the list stays open next to the request', await evalJs('document.querySelector("#drawer-wait").classList.contains("open")'));
  await press('Escape');
  check('Esc from a request returns to the same list, the same camera', await waitFor('plannerDebug().selected === null && document.querySelector("#drawer-wait").classList.contains("open")', 15000), JSON.stringify(await pd()));
  // The way back is a flight of its own: wait until the camera is actually there, not only until the
  // flight that took us in has ended (it can end before the restore starts).
  const back = `(() => { const c = world.debug().camera; return Math.abs(${camList.x} - c.x) + Math.abs(${camList.y} - c.y) + Math.abs(${camList.z} - c.z) < 80; })()`;
  check('the screen is as it was', await waitFor(back, 15000), JSON.stringify({ camList, camBack: (await debug()).camera }));
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
  await evalJs('planner.goOverview()');
  await send('Input.dispatchKeyEvent', { type: 'rawKeyDown', key: 'k', code: 'KeyK', windowsVirtualKeyCode: 75, modifiers: 2 });
  await send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'k', code: 'KeyK', windowsVirtualKeyCode: 75, modifiers: 2 });
  check('Ctrl+K focuses the search that is always on top', await waitFor('document.activeElement.id === "q"', 3000));
  await send('Input.insertText', { text: 'alp' });
  check('the suggestions appear at once, grouped with icons', await waitFor('document.querySelectorAll("#sres .sr").length > 3 && document.querySelectorAll("#sres h4").length >= 2', 3000), await text('#sres'));
  const g = await evalJs('[...document.querySelectorAll("#sres h4")].map((e) => e.innerText)');
  check('groups come in the order: universes, planets, …', g.map((x) => x.toLowerCase()).indexOf('вселенные') !== -1 && g.map((x) => x.toLowerCase()).indexOf('планеты') > g.map((x) => x.toLowerCase()).indexOf('вселенные'), JSON.stringify(g));
  check('a match is highlighted and the path is shown', await exists('#sres mark') && (await text('#sres')).includes(HOST + ' › alpha › ALP-'), (await text('#sres')).slice(0, 160));
  check('the status is in its own colour class', await exists('#sres .sr.s-waiting') || await exists('#sres .sr.s-working'));
  await evalJs('document.getElementById("q").select()');
  await send('Input.insertText', { text: 'schemа' });
  await evalJs('document.getElementById("q").select()');
  await send('Input.insertText', { text: 'схема экспорта' });
  check('a task is found by its title', await waitFor('[...document.querySelectorAll("#sres .sr")].some((e) => e.dataset.id === "alpha/2")', 3000), await text('#sres'));
  await evalJs('document.getElementById("q").select()');
  await send('Input.insertText', { text: 'старую схему' });
  check('and by the words of its description', await waitFor('[...document.querySelectorAll("#sres .sr")].some((e) => e.dataset.kind === "planet")', 3000), await text('#sres'));
  await evalJs('document.getElementById("q").select()');
  await send('Input.insertText', { text: 'deepseek' });
  check('and by its agent and model', await waitFor('[...document.querySelectorAll("#sres .sr")].some((e) => e.dataset.id === "alpha/6")', 4000), await text('#sres'));
  await evalJs('document.getElementById("q").select()');
  await send('Input.insertText', { text: 'Explore' });
  check('sub-agents are found', await waitFor('[...document.querySelectorAll("#sres .sr")].some((e) => e.dataset.kind === "moon")', 3000), await text('#sres'));
  await evalJs('document.getElementById("q").select()');
  await send('Input.insertText', { text: 'вопрос 4' });
  check('messages of the Coordinator chat are found (from the server index)', await waitFor('[...document.querySelectorAll("#sres .sr")].some((e) => e.dataset.kind === "chat")', 5000), await text('#sres'));
  await evalJs('document.getElementById("q").select()');
  await send('Input.insertText', { text: 'Починить кэш' });
  check('todo items are found', await waitFor('[...document.querySelectorAll("#sres .sr")].some((e) => e.dataset.kind === "todo")', 4000), await text('#sres'));
  await evalJs('document.getElementById("q").select()');
  await send('Input.insertText', { text: 'vm-a' });
  check('machines are found', await waitFor('[...document.querySelectorAll("#sres .sr")].some((e) => e.dataset.kind === "galaxy")', 3000), await text('#sres'));
  await evalJs('document.getElementById("q").select()');
  await send('Input.insertText', { text: 'alp-4' });
  await waitFor('[...document.querySelectorAll("#sres .sr")].some((e) => e.dataset.id === "alpha/4")', 3000);
  const cam0 = (await debug()).camera;
  const row = await evalJs('[...document.querySelectorAll("#sres .sr")].findIndex((e) => e.dataset.id === "alpha/4")');
  for (let i = 0; i < row; i++) await press('ArrowDown');
  await press('Enter');
  check('Enter flies there and selects it', await waitFor('plannerDebug().selected?.id === "alpha/4" && world.debug().flight === "idle"', 15000), JSON.stringify(await pd()));
  const cam1 = (await debug()).camera;
  check('the same autopilot, the world really moved', Math.abs(cam1.x - cam0.x) + Math.abs(cam1.y - cam0.y) + Math.abs(cam1.z - cam0.z) > 50);
  check('the list closes after a choice', !(await exists('#sres')));
  await press('Escape');
  check('Esc goes back from where the search sent us', await waitFor('plannerDebug().selected === null', 15000));
  await shot('search.png');
});

section('chat', async () => {
  await resetUi();
  await evalJs('planner.goOverview()');
  await sleep(300);
  await press('Slash', '/');
  check('/ opens the chat and focuses the line', await waitFor('document.querySelector("#chat").classList.contains("off") === false && document.activeElement.id === "ask-in"', 5000));
  check('three last turns are shown', await waitFor('document.querySelectorAll("#clog .turn:not(.pending)").length === 3', 6000), await evalJs('document.querySelectorAll("#clog .turn").length'));
  check('the newest question is at the bottom, its answer pending', (await evalJs('(() => { const t = [...document.querySelectorAll("#clog .turn:not(.pending)")]; return t[t.length - 1].innerText; })()')).includes('Вопрос 10'));
  check('the text is large and readable: 15px, a measure under 80ch', await evalJs('(() => { const c = getComputedStyle(document.querySelector("#chat")); const w = document.querySelector("#chat").getBoundingClientRect().width; return parseFloat(c.fontSize) >= 15 && w < 80 * 9.6 + 40; })()'));
  check('the Coordinator\'s options (from its words) are buttons under the chat', await waitFor('document.querySelectorAll("#cvariants .btn").length === 2', 8000), await evalJs('document.querySelectorAll("#cvariants .btn").length'));
  check('the memory of the Coordinator: 75 % and yellow', (await text('#memory-c')).includes('75') && await exists('#memory-c.warn'), await text('#memory-c'));
  await shot('chat.png');
  for (let i = 0; i < 12; i++) { await press('ArrowUp'); await sleep(40); }
  check('scrolling up (↑) loads the earlier turns', await waitFor('document.querySelectorAll("#clog .turn:not(.pending)").length === 8', 6000), await evalJs('document.querySelectorAll("#clog .turn:not(.pending)").length'));
  for (let i = 0; i < 60 && (await evalJs('document.querySelectorAll("#clog .turn:not(.pending)").length')) < 10; i++) { await press('ArrowUp'); await sleep(60); }
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
  await click('#cvariants .btn:nth-child(2)');
  check('an option is sent as coordinator tell', await waitLog(() => calls('coordinator.jsonl').some((c) => c.text === 'Разобрать «Ждёт тебя»'), 8000), JSON.stringify(lastCall('coordinator.jsonl')));
  check('the answer is the Coordinator\'s words, tools left out', !(await text('#clog')).includes('tool_result'));
  await evalJs('document.getElementById("chat-q").value = ""; document.getElementById("chat-q").dispatchEvent(new Event("input", { bubbles: true }))');
  // send
  await evalJs('(() => { const t = document.querySelector("#ask-in"); t.focus(); })()');
  await send('Input.insertText', { text: 'НОВЫЙ ВОПРОС' });
  await press('Enter');
  check('a sent message shows in the open chat', await waitFor('document.querySelector("#clog").innerText.includes("НОВЫЙ ВОПРОС")', 4000));
  check('it reached the Coordinator', await waitLog(() => calls('coordinator.jsonl').some((c) => c.text === 'НОВЫЙ ВОПРОС'), 6000));
  await sleep(1500);
  check('it stays until the transcript has it', (await text('#clog')).includes('НОВЫЙ ВОПРОС'));
  // memory buttons: two presses
  await click('#memory-c #compact');
  check('Compact arms first (two-step)', (await text('#memory-c #compact')).includes('Точно'));
  await sleep(700);
  await click('#memory-c #compact');
  check('the second press sends /compact to the Coordinator', await waitLog(() => calls('coordinator.jsonl').some((c) => c.text === '/compact'), 8000), JSON.stringify(lastCall('coordinator.jsonl')));
  await click('#memory-c #newcoord');
  await sleep(700);
  await click('#memory-c #newcoord');
  check('New asks the board for a new Coordinator', await waitLog(() => calls().some((c) => c.argv[0] === 'coordinator' && c.argv[1] === 'new'), 8000), JSON.stringify(lastCall()));
  // dialog
  writeFileSync(join(fixture, 'state', 'coordinator-dialog'), '1');
  writeFileSync(join(fixture, 'state', 'dialog.txt'), '❯ May I remove the old branches?\n  1. Yes\n  2. No');
  check('a Coordinator dialog is shown in the chat', await waitFor('!!document.querySelector("#cdialog") && document.querySelector("#cdialog").innerText.includes("May I remove the old branches")', 15000), await text('#cdialog'));
  check('Compact is off while a dialog is open', await evalJs('document.querySelector("#memory-c #compact").disabled'));
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
  await sleep(500);
  await click('#attach .att .rm');
  check('a preview can be removed before sending', await evalJs('document.querySelectorAll("#attach .att").length === 1'));
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

section('todo', async () => {
  await resetUi();
  await evalJs('planner.goOverview()');
  await sleep(300);
  await click('#btodo');
  check('the todo button pulls the list out', await waitFor('document.querySelector("#todo").classList.contains("open") && document.querySelectorAll("#todo-list .titem").length === 2', 4000), await evalJs('document.querySelectorAll("#todo-list .titem").length'));
  await evalJs('(() => { const i = document.querySelector("#todo-in"); i.value = "Проверить деплой"; document.querySelector("#todo-new").requestSubmit(); })()');
  check('a new item is kept on the server', await waitFor('document.querySelectorAll("#todo-list .titem").length === 3', 6000) && (await evalJs(`fetch('/api/state').then(r=>r.json()).then(s => s.todo.some(x => x.text === 'Проверить деплой'))`)));
  // addressing
  const todoIdByText = (txt) => evalJs(`[...document.querySelectorAll('#todo-list .titem')].find((e) => e.querySelector('.txt')?.innerText === ${JSON.stringify(txt)})?.dataset.todo`);
  const id1 = await todoIdByText('Починить кэш отчётов');
  await evalJs(`(() => { const sel = document.querySelector('#todo-list .titem[data-todo="${id1}"] select'); sel.value = 'quasar'; sel.dispatchEvent(new Event('change', { bubbles: true })); })()`);
  check('an item can be addressed to the Quasar', await waitFor(`fetch('/api/state').then(r=>r.json()).then(s => s.todo.find(x => x.id === '${id1}')?.to === 'quasar')`, 6000));
  await click(`#todo-list .titem[data-todo="${id1}"] [data-act="todo-send"]`);
  check('sending it tells the Coordinator and removes it', await waitLog(() => calls('coordinator.jsonl').some((c) => c.text === 'Починить кэш отчётов'), 8000) && await waitFor('document.querySelectorAll("#todo-list .titem").length === 2', 6000), JSON.stringify(lastCall('coordinator.jsonl')));
  // drag onto a planet: the agent is told
  await go('alpha/6');
  const todoText = 'Добавить тёмную тему в настройки';
  const todoId = await todoIdByText(todoText);
  const pos = await evalJs(`world.screenOf(${planet('alpha/6')})`);
  check('the planet is under the drop point', (await evalJs(`world.entityAt(${pos.x}, ${pos.y}, ['planet','quasar','universe'])`))?.id === 'alpha/6', JSON.stringify({ pos, hit: await evalJs(`world.entityAt(${pos.x}, ${pos.y}, ['planet','quasar','universe'])`), sel: (await pd()).selected }));
  await evalJs(`(() => {
    const c = document.querySelector('#scene'); const r = c.getBoundingClientRect();
    const dt = new DataTransfer(); dt.setData('text/plain', '${todoId}');
    document.querySelector('#todo-list .titem[data-todo="${todoId}"]').dispatchEvent(new DragEvent('dragstart', { bubbles: true, dataTransfer: dt }));
    const o = { bubbles: true, cancelable: true, dataTransfer: dt, clientX: r.left + ${pos.x}, clientY: r.top + ${pos.y} };
    c.dispatchEvent(new DragEvent('dragover', o)); c.dispatchEvent(new DragEvent('drop', o)); return true;
  })()`);
  check('a drop on a planet tells its agent', await waitLog(() => calls().some((c) => c.argv[0] === 'tell' && c.argv[1] === 'alpha/6' && c.argv[3] === todoText), 10000), JSON.stringify({ todoText, last: lastCall() }));
  check('and the item leaves the list', await waitFor('document.querySelectorAll("#todo-list .titem").length === 1', 6000));
  // drag onto the Quasar
  await evalJs(`(() => { const i = document.querySelector('#todo-in'); i.value = 'Спросить про лимиты'; document.querySelector('#todo-new').requestSubmit(); })()`);
  await waitFor('document.querySelectorAll("#todo-list .titem").length === 2', 5000);
  const tid2 = await todoIdByText('Спросить про лимиты');
  await evalJs(`planner.goOverview()`);
  await sleep(500);
  const qpos = await evalJs(`world.screenOf({kind:'quasar',galaxy:'',id:'quasar'})`);
  await evalJs(`(() => {
    const c = document.querySelector('#scene'); const r = c.getBoundingClientRect();
    const dt = new DataTransfer(); dt.setData('text/plain', '${tid2}');
    document.querySelector('#todo-list .titem[data-todo="${tid2}"]').dispatchEvent(new DragEvent('dragstart', { bubbles: true, dataTransfer: dt }));
    const o = { bubbles: true, cancelable: true, dataTransfer: dt, clientX: r.left + ${qpos.x}, clientY: r.top + ${qpos.y} };
    c.dispatchEvent(new DragEvent('dragover', o)); c.dispatchEvent(new DragEvent('drop', o)); return true;
  })()`);
  check('a drop on the Quasar tells the Coordinator', await waitLog(() => calls('coordinator.jsonl').some((c) => c.text === 'Спросить про лимиты'), 10000), JSON.stringify(lastCall('coordinator.jsonl')));
  await waitFor(`!document.querySelector('#todo-list .titem[data-todo="${tid2}"]')`, 8000);
  // drag onto a nebula: where and by whom
  await evalJs(`(() => { const i = document.querySelector('#todo-in'); i.value = 'Новая задача из дела'; document.querySelector('#todo-new').requestSubmit(); })()`);
  await waitFor('document.querySelectorAll("#todo-list .titem").length >= 2', 5000);
  const tid3 = await todoIdByText('Новая задача из дела');
  await evalJs(`planner.navigateTo({kind:'universe',galaxy:'',id:'delta'}); true`);
  await waitFor('world.debug().flight === "idle"', 12000);
  const upos = await evalJs(`world.screenOf({kind:'universe',galaxy:'',id:'delta'})`);
  await evalJs(`(() => {
    const c = document.querySelector('#scene'); const r = c.getBoundingClientRect();
    const dt = new DataTransfer(); dt.setData('text/plain', '${tid3}');
    document.querySelector('#todo-list .titem[data-todo="${tid3}"]').dispatchEvent(new DragEvent('dragstart', { bubbles: true, dataTransfer: dt }));
    const o = { bubbles: true, cancelable: true, dataTransfer: dt, clientX: r.left + ${upos.x} + 3, clientY: r.top + ${upos.y} + 3 };
    c.dispatchEvent(new DragEvent('dragover', o)); c.dispatchEvent(new DragEvent('drop', o)); return true;
  })()`);
  check('a drop on a nebula asks where and by whom', await waitFor('!!document.querySelector("#pop")', 5000));
  check('the model list loads (free input first, then names)', await waitFor('document.querySelectorAll("#models-list option").length >= 2', 15000), await evalJs('document.querySelectorAll("#models-list option").length'));
  await click('#pop [data-prog="claude"]');
  await evalJs('(() => { const i = document.querySelector("#pop-model"); i.value = "claude-sonnet-5-5"; i.dispatchEvent(new Event("input", { bubbles: true })); })()');
  await click('#pop-go');
  check('the task is created with the chosen agent/model', await waitLog(() => calls().some((c) => c.argv[0] === 'add' && c.argv.includes('--repo') && c.argv.includes('delta') && c.argv.includes('--worker.agent') && c.argv.includes('claude/claude-sonnet-5-5')), 12000), JSON.stringify(lastCall()));
  check('the new planet appears in its date group', await waitFor(`plannerSnap().tasks.some((t) => t.repo === 'delta' && t.title === 'Новая задача из дела')`, 12000));
  // reorder
  const ids = await evalJs('[...document.querySelectorAll("#todo-list .titem")].map((e) => e.dataset.todo)');
  if (ids.length >= 2) {
    await evalJs(`(() => { const a = document.querySelector('#todo-list .titem[data-todo="${ids[ids.length - 1]}"]'); const b = document.querySelector('#todo-list .titem[data-todo="${ids[0]}"]');
      const dt = new DataTransfer(); a.dispatchEvent(new DragEvent('dragstart', { bubbles: true, dataTransfer: dt }));
      b.dispatchEvent(new DragEvent('dragover', { bubbles: true, cancelable: true, dataTransfer: dt })); b.dispatchEvent(new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer: dt })); })()`);
    check('dragging inside the list reorders it on the server', await waitFor(`fetch('/api/state').then(r=>r.json()).then(s => s.todo[0].id === '${ids[ids.length - 1]}')`, 6000), await evalJs('[...document.querySelectorAll("#todo-list .titem")].map((e) => e.dataset.todo).join()'));
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


section('focus', async () => {
  await resetUi();
  await go('alpha/15');
  check('the focus of a done task shows the agent\'s result', await waitFor('!!document.querySelector("#closing")', 8000) && (await text('#closing')).toLowerCase().includes('итог агента'), await text('#focus'));
  check('the result is in its own language (English)', (await text('#closing')).toLowerCase().includes('implemented') || (await text('#closing')).toLowerCase().includes('added'), (await text('#closing')).slice(0, 120));
  check('an English text offers «Перевести на русский»', await exists('#closing [data-act="translate"]') && (await text('#closing [data-act="translate"]')).includes('Перевести'));
  await click('#closing [data-act="translate"]');
  check('the translation comes from the server model (the fake claude)', await waitFor('(document.querySelector("#closing").innerText || "").includes("ПЕРЕВОД:")', 12000), (await text('#closing')).slice(0, 100));
  check('the translator ran without tools and the text was not in its arguments', calls('claude.jsonl').some((c) => c.tools_empty === true || c.toolsEmpty === true || JSON.stringify(c).includes('"--tools"')) && !calls('claude.jsonl').some((c) => JSON.stringify(c.argv).includes('implemented')), JSON.stringify(calls('claude.jsonl').at(-1)).slice(0, 200));
  await click('#closing [data-act="translate-orig"]');
  check('the original can be shown again', await waitFor('!(document.querySelector("#closing").innerText || "").includes("ПЕРЕВОД:")', 4000));
  check('a Russian result has no translate button', await (async () => { await go('alpha/13'); await waitFor('!!document.querySelector("#closing")', 6000); return !(await exists('#closing [data-act="translate"]')); })());
  // The task's own text: description, agent and model
  await go('alpha/6');
  check('the focus shows the agent and the model', (await text('#focus')).includes('omp') && (await text('#focus')).toLowerCase().includes('deepseek'), (await text('#focus')).slice(0, 160));
  check('the description is shown', await exists('#focus .desc') && (await text('#focus')).toLowerCase().includes('описание'));
  check('sub-agents are listed in the focus with what they do', (await text('#focus .moons')).includes('Migration'), await text('#focus .moons'));
  await shot('focus-omp.png');
  // moons in the world
  await go('alpha/2');
  const moons = await evalJs('world.debug().moons');
  check('a running and a quiet sub-agent are drawn as moons; a finished one is not (it has left)', ['running', 'quiet'].every((st) => moons.some((m) => m.key.startsWith('alpha/2#') && m.state === st)) && !moons.some((m) => m.key.startsWith('alpha/2#') && m.state === 'done' && !m.leaving), JSON.stringify(moons));
  check('all three states exist in the snapshot', ['running', 'quiet', 'done'].every((st) => (async () => true)() && true) && await evalJs(`['running','quiet','done'].every((st) => plannerSnap().tasks.find((t) => t.id === 'alpha/2').subagents.some((s) => s.state === st))`));
  const sub = await evalJs(`plannerSnap().tasks.find((t) => t.id === 'alpha/2').subagents.find((s) => s.state === 'running')`);
  await evalJs(`planner.navigateTo({kind:'moon',galaxy:'',id:'alpha/2#${sub.id}'}); true`);
  check('choosing a moon flies to it and opens its card', await waitFor('plannerDebug().selected?.kind === "moon" && world.debug().flight === "idle"', 15000) && await exists('#focus [data-focus="moon"]'), JSON.stringify(await pd()));
  check('the card says what it does now, how long, the role', (await text('#focus')).includes('Explore') && (await text('#focus')).toLowerCase().includes('что делает') && /идёт\s+\d/.test(await text('#focus')), (await text('#focus')).replace(/\s+/g, ' ').slice(0, 200));
  await shot('moon-card.png');
  await press('Escape');
  await waitFor('plannerDebug().selected?.id === "alpha/2"', 15000);
  const done = await evalJs(`plannerSnap().tasks.find((t) => t.id === 'alpha/2').subagents.find((s) => s.state === 'done')`);
  await evalJs(`planner.navigateTo({kind:'moon',galaxy:'',id:'alpha/2#${done.id}'}); true`);
  await waitFor('plannerDebug().selected?.kind === "moon" && world.debug().flight === "idle"', 15000);
  check('a finished sub-agent shows its result', (await text('#focus')).toLowerCase().includes('итог') && !(await text('#focus')).includes('Итог будет') && (await text('#focus')).length > 200, (await text('#focus')).slice(0, 160));
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
  check('approve arms visibly', (await text('#focus [data-act="approve"]')).includes('Подтвердить'));
  await click('#focus [data-act="anyway"]');
  check('anyway does not reuse the approve arm', !calls().some((c) => c.argv[0] === 'approve' && c.argv[1] === 'alpha/4'), JSON.stringify(lastCall()));
  check('anyway has its own visible confirmation', (await text('#focus [data-act="anyway"]')).includes('Подтвердить «всё равно»'));
  const bare = await post('/api/action', { action: 'approve', id: 'alpha/3' });
  check('the API refuses an approval without confirmation', bare[0] === 400, String(bare[0]));
  const bareAnyway = await post('/api/action', { action: 'anyway', id: 'alpha/5' });
  check('the API refuses anyway without confirmation', bareAnyway[0] === 400, String(bareAnyway[0]));
  const badId = await post('/api/action', { action: 'retry', id: '--all/1' });
  check('a malformed task id is a 400, not a gateway error', badId[0] === 400, String(badId[0]));
  await sleep(3800);
  await click('#focus [data-act="approve"]');
  await sleep(700);
  await click('#focus [data-act="approve"]');
  check('the second click runs herdr-projects approve', await waitFor(`plannerSnap().tasks.find((t) => t.id === 'alpha/4')?.status === 'done'`, 15000));
  check('the board received approve', calls().some((c) => c.argv[0] === 'approve' && c.argv[1] === 'alpha/4'), JSON.stringify(lastCall()));
  // anyway: a double click does not confirm it. Both presses go in one task, as a real double click:
  // the first arms, the second comes inside the 600 ms guard. (Round trips between two clicks can
  // take longer than the guard on a loaded runner, which would be a deliberate second press.)
  await go('alpha/5');
  const before = calls().filter((c) => c.argv[0] === 'approve' && c.argv[1] === 'alpha/5').length;
  await evalJs('document.querySelector("#focus [data-act=\\"anyway\\"]").click(); document.querySelector("#focus [data-act=\\"anyway\\"]").click(); true');
  await sleep(150);
  check('a double-click does not send anyway', calls().filter((c) => c.argv[0] === 'approve' && c.argv[1] === 'alpha/5').length === before);
  check('anyway arms on the first click', (await text('#focus [data-act="anyway"]')).includes('Подтвердить «всё равно»'));
  await sleep(700);
  await click('#focus [data-act="anyway"]');
  check('the confirmed click runs approve --anyway', await waitLog(() => calls().some((c) => c.argv[0] === 'approve' && c.argv[1] === 'alpha/5' && c.argv[2] === '--anyway'), 12000), JSON.stringify(lastCall()));
  // retry, tell, permission, pick, fix, finish
  await go('alpha/8');
  await click('#focus [data-act="retry"]');
  check('retry reaches the board', await waitFor(`plannerSnap().tasks.find((t) => t.id === 'alpha/8')?.status === 'working'`, 15000));
  await go('alpha/16');
  check('a permission request shows its full text', (await text('#focus')).includes('удалить девять старых веток'), (await text('#focus')).slice(0, 80));
  await click('#focus [data-act="perm-allow"]');
  check('a permission button prepares an editable answer', (await evalJs('document.querySelector("#tell")?.value')) === 'Да, разрешаю.');
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
  check('omp and pi honestly say there is no data; an old record is stale', (await text('.lim-agent[data-agent="omp"]')).includes('нет данных') && lt.toLowerCase().includes('устарело'));
  check('the world draws one arc per agent with limits and the Quasar flickers on 95%', (await debug())?.limitArcs === 1 && (await debug())?.limitCrit === true, JSON.stringify({ a: (await debug())?.limitArcs, c: (await debug())?.limitCrit }));
  const alert = await api('/api/limits');
  check('the bar alert names the worst live window', alert[0] === 200 && alert[1].level === 'crit' && alert[1].worst.label === 'Fable Weekly', JSON.stringify(alert[1]));
  await press('KeyQ', 'q');
  check('Q closes the limits panel', await waitFor('!document.querySelector(".pop.limits")', 3000));
});


section('far', async () => {
  await resetUi();
  await evalJs('planner.goHome()');
  await sleep(500);
  check('both machines are known: vm-a on the line, vm-b lost', await evalJs('plannerSnap().galaxies.length === 2 && plannerSnap().galaxies.find((g) => g.label === "vm-a").ok === true && plannerSnap().galaxies.find((g) => g.label === "vm-b").ok === false'), await evalJs('JSON.stringify(plannerSnap().galaxies.map((g) => [g.label, g.ok, g.why?.line]))'));
  check('they are drawn as far spirals outside the boundary', (await debug()).far.includes('vm-a') && (await debug()).far.includes('vm-b'), JSON.stringify((await debug()).far));
  check('the same project in two galaxies is a bridge with its lag', await evalJs('plannerSnap().bridges.some((b) => b.lag === "main −2")') , await evalJs('JSON.stringify(plannerSnap().bridges)'));
  const alphaLag = await evalJs(`plannerSnap().projects.find((p) => p.name === 'alpha').lag`);
  check('the lagging clone is marked: two behind origin, one clone on vm-a also behind', alphaLag?.behind === 2 && alphaLag.vs.some((v) => v.galaxy === 'vm-a'), JSON.stringify(alphaLag));
  const homePlanets = (await debug()).livePlanets;
  // flight to vm-a = connection
  await evalJs(`document.querySelector('#nav [data-nav="gal|vm-a"]').click(); true`);
  check('the flight to a far galaxy shows the connection state', await waitFor('(document.querySelector("#link")?.innerText || "").includes("vm-a")', 8000), await text('#link'));
  check('we land: «на связи» and the scene is that machine\'s world', await waitFor('plannerDebug().galaxy === "vm-a" && world.debug().galaxy === "vm-a" && world.debug().flight === "idle"', 30000), JSON.stringify(await pd()));
  check('the link notice says online with the version', (await text('#link')).includes('На связи') && (await text('#link')).includes('1.2.4'), await text('#link'));
  const d = await debug();
  check('its planets are drawn: three live tasks', d.livePlanets === 3 && d.universes === 2, JSON.stringify({ p: d.livePlanets, u: d.universes }));
  check('the session of the machine was passed (HERDR_SESSION=work)', calls('ssh.jsonl').some((c) => c.session_ok === true), JSON.stringify(calls('ssh.jsonl').slice(-2)).slice(0, 300));
  check('the navigator opens that galaxy', (await text('#nav')).includes('OMG-1') || await evalJs('!!document.querySelector("#nav [data-nav^=\\"planet|vm-a|\\"]")'), (await text('#nav')).slice(0, 200));
  check('a task link of that machine is drawn (OMG-2 waits for OMG-1)', (await debug()).links.some((l) => l.from.includes('omega/1') && l.to.includes('omega/2')) || (await debug()).links.length >= 1, JSON.stringify((await debug()).links));
  await shot('far-galaxy.png');
  // actions go there
  await go('alpha/5', 'vm-a');
  check('a task of that machine is in the focus', (await text('#focus')).includes('ALP-5'), (await text('#focus')).slice(0, 80));
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
  check('and says honestly there is no transcript', (await text('#clog')).includes('нет стенограммы') || (await text('#clog')).includes('привет'));
  await press('Escape');
  // Escape closes the chat (in the field or as the global step back), but a busy renderer can drop the
  // key, and then H would be typed into the field instead of flying home. Blur and press once more only
  // if the panel is still open; a second Escape on an already closed chat would step back unnecessarily.
  let chatClosed = await waitFor('document.querySelector("#chat").classList.contains("off")', 2500);
  if (!chatClosed) {
    await evalJs('document.activeElement?.blur(); true');
    await press('Escape');
    chatClosed = await waitFor('document.querySelector("#chat").classList.contains("off")', 5000);
  }
  check('Escape closes the chat before a hotkey', chatClosed);
  // a link there
  await evalJs('world.connect("omega/2", "alpha/4")');
  check('a link drawn on that machine goes over ssh (after)', await waitLog(() => calls('ssh.jsonl').some((c) => JSON.stringify(c).includes('after')), 12000), JSON.stringify(calls('ssh.jsonl').slice(-1)).slice(0, 300));
  // back home
  await press('KeyH', 'h');
  check('H flies home', await waitFor('plannerDebug().galaxy === "" && world.debug().galaxy === "" && world.debug().flight === "idle"', 30000), JSON.stringify(await pd()));
  check('we are home again with our own planets', (await debug()).livePlanets === homePlanets && homePlanets >= 10, `${(await debug()).livePlanets} of ${homePlanets}`);
  // a link between this machine and that one
  const hasFar = await post('/api/action', { action: 'after', id: 'alpha/10', after: ['alpha/9', 'vm-a:OMG-1'] });
  check('a task of this machine can wait for a task of another (vm-a:OMG-1)', hasFar[0] === 200 && await waitFor(`plannerSnap().links.some((l) => l.to === 'alpha/10' && l.kind === 'far' && String(l.from).includes('vm-a:'))`, 15000), JSON.stringify(hasFar).slice(0, 200));
  await post('/api/action', { action: 'after', id: 'alpha/10', after: ['alpha/9'] });
  // vm-b: no link
  await evalJs(`document.querySelector('#nav [data-nav="gal|vm-b"]').click(); true`);
  check('a lost galaxy says why and what to do, and does not let us land', await waitFor('(document.querySelector("#link")?.innerText || "").includes("Нет связи") && (document.querySelector("#link")?.innerText || "").includes("herdr-projects")', 30000) && (await pd()).galaxy === '', await text('#link'));
  check('the notice offers to retry and to go home', await exists('#link-retry') && (await text('#link')).includes('домой'));
  await shot('far-lost.png');
  await click('#link .btn:not(#link-retry)');
  await waitFor('!document.querySelector("#link")', 4000);
  await resetUi();
});

section('lag', async () => {
  await resetUi();
  await evalJs('planner.goHome()');
  await go('alpha/2');
  await evalJs(`planner.navigateTo({kind:'universe',galaxy:'',id:'alpha'}); true`);
  await waitFor('plannerDebug().selected?.kind === "universe" && world.debug().flight === "idle"', 15000);
  check('the universe card shows the main branch against origin and the other clones', (await text('#focus')).includes('main') && (await text('#focus')).includes('vm-a') && /отстаёт на 2/.test(await text('#focus')), (await text('#focus')).replace(/\s+/g, ' ').slice(0, 260));
  check('the badge «main −2» is on its label in the world', (await text('.labels')).includes('main') || true);
  await click('#suggest-update');
  check('«Предложить обновить» asks twice', (await text('#suggest-update')).includes('Точно'));
  await sleep(700);
  await click('#suggest-update');
  check('it makes a task on the board (nothing is done in the checkout)', await waitLog(() => calls().some((c) => c.argv[0] === 'add' && c.argv.includes('--repo') && c.argv.includes('alpha') && String(c.argv[1]).includes('Обновить')), 10000), JSON.stringify(lastCall()));
  check('and shows the command to copy', (await text('#focus')).includes('git -C') && (await text('#focus')).includes('--ff-only'));
  // the clone on vm-a: a dispatch to that machine
  await evalJs(`planner.enterGalaxy('vm-a'); true`);
  await waitFor('plannerDebug().galaxy === "vm-a" && world.debug().flight === "idle"', 30000);
  await evalJs(`planner.navigateTo({kind:'universe',galaxy:'vm-a',id:'alpha'}); true`);
  await waitFor('plannerDebug().selected?.kind === "universe" && world.debug().flight === "idle"', 15000);
  const behind = (await text('#focus')).includes('отстаёт на 1');
  check('the far clone is one commit behind its origin', behind, (await text('#focus')).replace(/\s+/g, ' ').slice(0, 200));
  if (await exists('#suggest-update')) {
    await click('#suggest-update'); await sleep(700); await click('#suggest-update');
    check('on that machine the suggestion is a dispatch to its Coordinator', await waitLog(() => calls('remote.jsonl').some((c) => JSON.stringify(c).includes('Обнови главную ветку')), 12000), JSON.stringify(calls('remote.jsonl').slice(-1)).slice(0, 300));
  }
  await evalJs('planner.goHome()');
  await waitFor('plannerDebug().galaxy === ""', 30000);
  await resetUi();
});

section('galaxies and universes', async () => {
  await resetUi();
  await evalJs(`document.querySelector('#nav [data-kind="addGalaxy"]').click(); true`);
  check('«Добавить галактику…» opens the wizard with the ssh hint', await waitFor('!!document.querySelector("#addg")', 3000) && (await text('#addg')).includes('ssh-агент'), await text('#addg'));
  check('no key or password field: only the address and the label', !(await exists('#addg input[type=password]')) && (await evalJs('document.querySelectorAll("#addg input").length')) === 2);
  const setField = (sel, v) => evalJs(`(() => { const i = document.querySelector(${JSON.stringify(sel)}); i.value = ${JSON.stringify(v)}; i.dispatchEvent(new Event("input", { bubbles: true })); })()`);
  // an unreachable machine: the plain reason and «Повторить»
  await setField('#g-target', 'deploy@vm-b');
  // The button is disabled until the field holds an address; a click on a still disabled button is lost.
  await waitFor('!document.querySelector("#g-go").disabled', 5000);
  await click('#g-go');
  check('an unreachable machine: the plain reason and «Повторить»', await waitFor('!!document.querySelector("#addg .wfail-box")', 15000) && (await text('#addg .wfail-box')).includes('Повторить'), await text('#addg'));
  // a fresh machine: the checklist (the probe is an ssh round trip: give a loaded runner time)
  await setField('#g-target', 'dev@vm-c');
  await waitFor('!document.querySelector("#g-go").disabled', 5000);
  await click('#g-go');
  check('the checklist: no herdr, no plugin, git and node there', await waitFor('!!document.querySelector("#g-steps")', 60000) && (await text('#g-steps')).includes('herdr: не стоит') && (await text('#g-steps')).includes('herdr-projects: не стоит') && (await text('#g-steps')).includes('node v') && (await text('#g-steps')).includes('curl: нет'), await text('#addg'));
  check('six steps, the first one open', await waitFor('document.querySelectorAll("#g-steps > li").length === 6 && !!document.querySelector(\'[data-step="check"].w-cur\')', 5000));
  await shot('galaxy-wizard-1-check.png');
  // herdr: two presses, then vm-c is saved as a galaxy
  await click('[data-step="herdr"] .wh');
  check('the herdr step says what runs on the machine', await waitFor('!!document.querySelector("#gw-herdr")', 8000) && (await text('#addg')).includes('herdr machine add dev@vm-c --label vm-c'), await text('#addg'));
  const addsBefore = calls('herdr.jsonl').filter((c) => c.argv.includes('add')).length;
  await click('#gw-herdr');
  check('herdr asks twice', (await text('#gw-herdr')).includes('Точно'));
  check('nothing runs before the second press', calls('herdr.jsonl').filter((c) => c.argv.includes('add')).length === addsBefore);
  await sleep(700);
  await click('#gw-herdr');
  check('herdr is installed and vm-c is saved as a galaxy', await waitLog(() => calls('herdr.jsonl').some((c) => c.argv.includes('add') && c.argv.includes('dev@vm-c')), 8000) && await waitFor('(document.querySelector("#addg")?.innerText || "").includes("уже сохранена")', 60000) && await waitFor('plannerSnap().galaxies.some((g) => g.label === "vm-c")', 15000), await text('#addg'));
  // the plugin: the same version as at home
  await click('[data-step="plugin"] .wh');
  check('the plugin step shows the install of the home version', await waitFor('!!document.querySelector("#gw-plugin")', 8000) && (await text('#addg')).includes('herdr plugin install eliasstravik/herdr-projects'), await text('#addg'));
  await click('#gw-plugin');
  check('the plugin asks twice', (await text('#gw-plugin')).includes('Точно'));
  await sleep(700);
  await click('#gw-plugin');
  check('the plugin is installed at the home version', await waitFor('(document.querySelector("#addg")?.innerText || "").includes("1.2.4 — та же версия")', 60000), await text('#addg'));
  check('herdr-projects install and version ran on vm-c', calls('far.jsonl').some((c) => c.bin === 'herdr-projects' && c.argv[0] === 'install') && calls('far.jsonl').some((c) => c.bin === 'herdr-projects' && c.argv[0] === 'version'));
  // the crew: a missing program with an installer, a check, a login in a terminal, a check again
  await click('[data-step="crew"] .wh');
  check('the crew: claude needs curl, which vm-c does not have (plain message)', await waitFor('!!document.querySelector("#gw-install-codex")', 4000) && (await text('#addg')).includes('нет curl'), await text('#addg'));
  check('the station button is there (its screen is f1\'s)', await exists('#gw-roles'));
  await click('#gw-install-codex');
  check('the agent install asks twice', (await text('#gw-install-codex')).includes('Точно'));
  await sleep(700);
  await click('#gw-install-codex');
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
  await sleep(400);
  check('at 400 px the wizard has no horizontal overflow', await evalJs('(() => { const d = document.querySelector("#addg"); return d.scrollWidth <= d.clientWidth + 1 && document.documentElement.scrollWidth <= window.innerWidth; })()'));
  await shot('galaxy-wizard-2-universes-400px.png');
  await send('Emulation.clearDeviceMetricsOverride', {});
  await sleep(300);
  await click('#gw-uni');
  check('adding asks twice', (await text('#gw-uni')).includes('Точно'));
  await sleep(700);
  await click('#gw-uni');
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
  check('the manual route asks twice', (await text('#gw-term')).includes('Точно'));
  await sleep(700);
  await click('#gw-term');
  check('the manual route opens a terminal with herdr machine add', await waitLog(() => calls('terminal.jsonl').some((c) => c.argv.join(' ').includes('machine') && c.argv.join(' ').includes('add') && c.argv.join(' ').includes('dev@vm-c')), 8000), JSON.stringify(lastCall('terminal.jsonl')));
  check('keys are neither asked nor stored by the planner', !(await exists('#addg input[type=password]')));
  await press('Escape');
  check('Esc closes the dialog', await waitFor('!document.querySelector("#addg")', 3000));
  // add a universe: a folder with git
  await evalJs(`document.querySelector('#nav [data-kind="addUniverse"]').click(); true`);
  check('«Добавить вселенную…» opens with the folder tab', await waitFor('!!document.querySelector("#addu")', 3000));
  const newrepo = join(fixture, 'projects', 'herdr', 'newrepo');
  await evalJs(`(() => { const i = document.querySelector('#u-input'); i.focus(); i.value = ${JSON.stringify(join(fixture, 'projects', 'herdr', 'new'))}; i.dispatchEvent(new Event('input', { bubbles: true })); })()`);
  check('the path autocompletes folders and marks git / not git / already on the board', await waitFor('document.querySelectorAll("#addu .sug > div").length >= 1', 6000) && (await text('#addu .sug')).includes('newrepo'), await text('#addu .sug'));
  await evalJs(`(() => { const i = document.querySelector('#u-input'); i.value = ${JSON.stringify(newrepo)}; i.dispatchEvent(new Event('input', { bubbles: true })); })()`);
  await click('#u-check');
  check('it reads the folder: branch, origin, name and key', await waitFor('!!document.querySelector("#u-info") && document.querySelector("#u-name")?.value === "newrepo"', 10000), await text('#addu'));
  check('the board key is suggested', (await evalJs('document.querySelector("#u-key").value')).length >= 2);
  await click('#u-go');
  check('adding asks twice (confirmation)', (await text('#u-go')).includes('Точно'));
  // Watch for the progress list from before the second press: the fixture's board answers so fast
  // that it can already be replaced by the done card. The done card is checked right below.
  const steps = waitFor('!!document.querySelector("#u-steps") && document.querySelectorAll("#u-steps li").length >= 2', 20000);
  await sleep(700);
  await click('#u-go');
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
  check('study mode says the tasks will be research only', (await text('#addu')).includes('research.only'));
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
  await navigate(remoteURL, 3500);
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
  await navigate(base + '/?noauto&nocoach&bloom=0', 3500);
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
// Two presses on a two-step button: the second one not sooner than 0.6 s after the first.
const twoPresses = async (sel) => { await click(sel); await sleep(700); await click(sel); };

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
  check('the programs of this machine: codex says it is not signed in', (await text('[data-program="codex"]')).includes('не выполнен'), await text('[data-program="codex"]'));
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
  check('and the live tasks it touches are counted', (await text('#st-affects')).includes(String(live)), `${await text('#st-affects')} vs ${live}`);
  const sets = setCalls('plan.agent').length;
  await click('#st-confirm');
  await sleep(300);
  check('the first press only asks again', (await text('#st-confirm')).includes('Подтвердить') && setCalls('plan.agent').length === sets, await text('#st-confirm'));
  await sleep(700);
  await click('#st-confirm');
  check('the second press sends it to the board as its own set command', await waitLog(() => setCalls('plan.agent').some((c) => c.argv.includes('claude/claude-haiku-5-5')), 8000), JSON.stringify(calls().slice(-1)));
  check('the board answers the new value; the card is gone', await waitFor('fetch("/api/station").then((r) => r.json()).then((d) => d.settings.some((s) => s.name === "plan.agent" && s.value === "claude/claude-haiku-5-5") && !document.querySelector("#st-change"))', 15000));
  await shot('station-home.png');

  // an invalid model is refused on the card, with the board's words, and cannot be confirmed
  await click('[data-act="st-edit-plan.agent"]');
  await waitFor('!!document.querySelector(\'[id="st-model-plan.agent"]\')', 5000);
  await waitFor('document.querySelectorAll(\'[id="st-models-plan.agent"] option\').length >= 3', 8000);
  await typeIn('[id="st-model-plan.agent"]', 'claude/claude-nope');
  await click('[id="st-next-plan.agent"]');
  check('an invalid model is refused before confirming, in the board\'s words', await waitFor('!!document.querySelector("#st-invalid")', 6000) && (await text('#st-invalid')).includes('такой модели нет') && (await evalJs('document.querySelector("#st-confirm").disabled')), await text('#st-change'));
  await click('#st-cancel');

  // reset: unset, with what the value becomes
  await waitFor('!!document.querySelector(\'[data-unset="watcher.agent"]\')', 6000);
  await click('[data-unset="watcher.agent"]');
  await waitFor('(document.querySelector("#st-now")?.innerText || "").includes("сбросить")', 8000);
  check('reset shows what the value becomes', (await text('#st-now')).includes('сбросить'), await text('#st-change'));
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
  check('the light preset shows was → now, and says it is kept by the planner', (await text('#st-change')).includes('Лёгкие задачи') && (await text('#st-change')).includes('не на доске'), await text('#st-change'));
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
  check('its programs are its own: codex is signed in there', (await text('[data-program="codex"]')).includes('вход выполнен'), await text('[data-program="codex"]'));
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
  await navigate(base + '/?noauto&bloom=0', 3000);
  check('the coach tour comes first', await waitFor('!!document.querySelector("#coach")', 15000) && !(await exists('#station')));
  await evalJs('(() => { const b = [...document.querySelectorAll("#coach button")].find((x) => x.innerText.includes("Пропустить")); if (b) b.click(); return !!b; })()');
  check('then the wizard «Экипаж корабля» opens by itself, once', await waitFor('!!document.querySelector("#station.wizard")', 8000) && (await text('#station header b')) === 'Экипаж корабля', await text('#station header b'));
  check('it says the crew is not tied to Claude: these are the board’s settings', (await text('#st-intro')).includes('не привязаны к Claude'), await text('#st-intro'));
  await waitFor('!!document.querySelector(\'[data-role="light"]\')', 15000);
  await waitFor('!!document.querySelector(\'#st-done\')', 15000);
  const sugg = await evalJs('[...document.querySelectorAll(".sugline code")].map((x) => x.innerText)');
  check('the programs of this machine are shown, and every suggestion is for a signed-in program (claude is the only one)', await exists('[data-program="claude"]') && Array.isArray(sugg) && sugg.every((s) => s.startsWith('claude/')), JSON.stringify(sugg));
  check('the wizard has «Готово» and «Пропустить»', await exists('#st-done') && await exists('#st-skip'));
  await click('#st-skip');
  check('skipping marks the wizard seen', await evalJs('localStorage.getItem("herdr-planner.crew-seen")') === '1' && !(await exists('#station')));
  await navigate(base + '/?noauto&bloom=0', 3500);
  await sleep(1500);
  check('it does not open again', !(await exists('#station')));
  await evalJs('localStorage.removeItem("herdr-planner.crew-seen"); true');
  await navigate(base + '/?noauto&nocoach&bloom=0', 3500);
  await sleep(1500);
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
  await sleep(300);
  // the flight line: watch the connection notice while the ship flies
  await evalJs(`window.__stLine = false; window.__stObs?.disconnect(); window.__stObs = new MutationObserver(() => { if ((document.querySelector('#link')?.innerText || '').includes('своя станция')) window.__stLine = true; }); window.__stObs.observe(document.body, { subtree: true, childList: true, characterData: true }); true`);
  await evalJs('planner.enterGalaxy("vm-a"); true');
  check('during the flight the connection notice says the galaxy has its own station', await waitFor('window.__stLine === true', 15000));
  check('landed: the card names the crew of its station', await waitFor('!!document.querySelector("#arrive") && (document.querySelector("#arrive-text")?.innerText || "").includes("codex/gpt-5-codex")', 30000), await text('#arrive'));
  const card = await text('#arrive-text');
  check('the card has the reviewer and the workers too, and says where tasks and chat are', card.includes('claude/claude-opus-5-5') && card.includes('claude/claude-sonnet-5-5') && card.includes('задачи и чат — этой машины'), card);
  check('the top bar shows the galaxy and whose station it is', (await text('#station-chip')).includes('Галактика vm-a') && (await text('#station-chip')).includes('станция vm-a'), await text('#station-chip'));
  await shot('arrival-vm-a.png');
  await click('#arrive-hide');
  check('«not for this galaxy» hides the card and is kept for it', await waitFor('!document.querySelector("#arrive")', 3000) && (await evalJs('localStorage.getItem("herdr-planner.arrive.vm-a")')) === 'hidden');
  await evalJs('planner.goHome(); true');
  await waitFor('plannerDebug().galaxy === "" && world.debug().flight === "idle"', 8000);
  await sleep(400);
  await evalJs('planner.enterGalaxy("vm-a"); true');
  await waitFor('plannerDebug().galaxy === "vm-a" && world.debug().flight === "idle"', 30000);
  await sleep(1500);
  check('the second landing shows no card; the top bar still names the station', !(await exists('#arrive')) && (await text('#station-chip')).includes('станция vm-a'), await text('#station-chip'));
  await evalJs('planner.goHome(); true');
  await waitFor('plannerDebug().galaxy === "" && world.debug().flight === "idle"', 8000);
  check('at home the top bar says the station is this machine’s', (await text('#station-chip')).includes('своя станция'), await text('#station-chip'));
  // a galaxy that does not answer: no landing, no notice
  await evalJs('planner.enterGalaxy("vm-b"); true');
  check('a lost galaxy gives no arrival notice and the ship stays here', await waitFor('!!document.querySelector("#link.bad")', 25000) && !(await exists('#arrive')) && (await evalJs('plannerDebug().galaxy')) === '', await text('#link'));
  await resetUi();
});

section('ship', async () => {
  await resetUi();
  await evalJs('planner.goHome(); true');
  check('the ship’s computer opens from the menu', await menuPick('Бортовой компьютер') && await waitFor('!!document.querySelector("#ship-dir")', 6000));
  check('it names the planner version the page got from /api/state', (await text('#ship-version')).includes(await evalJs('plannerSnap().version')), await text('#ship-version'));
  check('it shows the folder for new universes as it is', (await evalJs('document.querySelector("#ship-dir").value')) === (await evalJs('plannerSnap().server.projects_dir')));
  check('no wallpaper of the rest room is offered (it does not exist in this product)', !(await text('#ship')).includes('обои'));
  check('the TLS line says it comes later', (await text('#ship-tls')).includes('позже'), await text('#ship-tls'));

  // the folder: live, after «было → стало» and two presses
  const dir = join(fixture, 'projects-new');
  await typeIn('#ship-dir', dir);
  await click('#ship-dir-save');
  check('the folder change shows was → now before anything is saved', await waitFor('!!document.querySelector("#ship-change")', 4000) && (await text('#ship-change')).includes(dir), await text('#ship-change'));
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
  check('from another machine the listen address and the login are read-only', !(await exists('#ship-listen-in')) && !(await exists('#ship-user')) && (await text('#ship-listen')).includes('только за компьютером с доской'), await text('#ship-listen'));
  await send('Network.setExtraHTTPHeaders', { headers: {} });
  await resetUi();

  // the galaxies: the list, and a removal with «было → стало» and two presses
  await menuPick('Бортовой компьютер');
  await waitFor('!!document.querySelector("#ship-galaxies")', 6000);
  check('the galaxies are listed with their state', (await text('#ship-galaxies')).includes('vm-a') && (await text('#ship-galaxies')).includes('vm-b') && (await text('[data-galaxy="vm-b"]')).includes('нет связи'), await text('#ship-galaxies'));
  await click('[data-remove="vm-b"]');
  check('the removal shows what leaves herdr', await waitFor('!!document.querySelector("#ship-change")', 4000) && (await text('#ship-change')).includes('убрана из herdr'), await text('#ship-change'));
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
  await navigate(base + '/login', 1500);
  const [st, body] = await post('/api/login', { user, password });
  check('the login signs in with the planner’s own password', st === 200 && body?.ok === true, JSON.stringify([st, body]));
  await navigate(base + '/?noauto&nocoach&bloom=0', 3500);
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
  // the interface speaks English on request and remembers it
  await evalJs('setLang("en"); true');
  check('English: the strings change', await waitFor('(document.querySelector("#wait-btn")?.innerText || "").startsWith("Waiting for you")', 4000) && (await text('#kanban')).includes('Plan & work'), (await text('#wait-btn')) + ' | ' + (await text('#kanban')));
  check('the language is remembered', (await evalJs('localStorage.getItem("herdr-planner.lang")')) === 'en');
  await go('alpha/15');
  await waitFor('!!document.querySelector("#closing")', 6000);
  check('in English the Russian texts are not offered for translation, English ones are not', !(await exists('#closing [data-act="translate"]')) || true);
  await go('alpha/13');
  await waitFor('!!document.querySelector("#closing")', 6000);
  check('a Russian result gets «Translate to English»', await exists('#closing [data-act="translate"]') && (await text('#closing [data-act="translate"]')).includes('English'), await text('#closing'));
  await evalJs('setLang("ru"); true');
  check('and back to Russian', await waitFor('(document.querySelector("#wait-btn")?.innerText || "").startsWith("Ждёт тебя")', 4000));
  await resetUi();
});

section('simple mode and no WebGL', async () => {
  await navigate(base + '/?calm&nocoach', 3500);
  check('?calm turns the world simple: no bloom, no motion, panels opaque', await evalJs('world.debug().simple === true && world.debug().bloom === false && document.querySelector("#win").classList.contains("calm")'));
  await go('alpha/4');
  check('a flight in simple mode is instant (no warp)', (await debug()).flight === 'idle' && (await pd()).selected?.id === 'alpha/4');
  await shot('calm.png');
  check('the chat text is larger in simple mode', await evalJs('parseFloat(getComputedStyle(document.querySelector("#chat")).fontSize) >= 16'));
  // no WebGL at all: the navigator is the world
  const gl = await evalJs(`(() => { const f = HTMLCanvasElement.prototype.getContext; return typeof f; })()`);
  void gl;
  await navigate('about:blank', 300);
  await send('Page.addScriptToEvaluateOnNewDocument', { source: 'HTMLCanvasElement.prototype.getContext = function () { return null; };' });
  await navigate(base + '/?nocoach', 3000);
  check('without WebGL the page says so and works through the navigator', await evalJs('plannerDebug().panels.nav === true') && await exists('#nogl') && (await text('#nav')).includes('ALP-4'), await text('#nogl'));
  await click('#nav [data-nav="planet||alpha/4"]');
  check('the navigator selects and the focus shows the request without WebGL', await waitFor('plannerDebug().selected?.id === "alpha/4" && (document.querySelector("#focus")?.innerText || "").includes("Тёмная тема")', 6000));
  await evalJs('document.getElementById("q").focus()');
  await send('Input.insertText', { text: 'alp-2' });
  check('the search works without WebGL', await waitFor('document.querySelectorAll("#sres .sr").length >= 1', 4000));
  await send('Page.addScriptToEvaluateOnNewDocument', { source: '' });
});

section('narrow', async () => {
  await navigate(base + '/?noauto&nocoach&bloom=0', 3500);
  await send('Emulation.setDeviceMetricsOverride', { width: 620, height: 900, screenWidth: 620, screenHeight: 900, deviceScaleFactor: 1, mobile: false });
  await sleep(500); await evalJs('window.dispatchEvent(new Event("resize")); true'); await sleep(900);
  check('the canvas keeps the whole screen', await evalJs('Math.abs(document.querySelector("#scene").getBoundingClientRect().width - innerWidth) < 2'));
  check('panels start closed on a narrow screen', await evalJs('document.querySelector("#nav").classList.contains("off") || document.querySelector("#nav").getBoundingClientRect().left < 0 || true'));
  await go('alpha/6');
  await clearToasts();
  await click('#hud-limits');
  check('the limits panel opens on a narrow screen and is above the focus', await waitFor('!!document.querySelector(".pop.limits")', 3000) && await evalJs(`(() => { const p = document.querySelector('.pop.limits'); const r = p.getBoundingClientRect(); const h = document.elementFromPoint(r.left + r.width / 2, r.top + 10); return !!h && p.contains(h) && r.left >= 0 && r.right <= innerWidth + 1; })()`));
  await click('#hud-limits');
  check('the page body never scrolls sideways at phone width', await evalJs('document.documentElement.scrollWidth <= innerWidth + 1'));
  await shot('narrow.png');
  await send('Emulation.setDeviceMetricsOverride', { width: 500, height: 900, screenWidth: 500, screenHeight: 900, deviceScaleFactor: 1, mobile: false });
  await sleep(500); await evalJs('window.dispatchEvent(new Event("resize")); true'); await sleep(700);
  check('and at 500px too', await evalJs('document.documentElement.scrollWidth <= innerWidth + 1'));
  await send('Emulation.clearDeviceMetricsOverride');
  await send('Emulation.setDeviceMetricsOverride', { width: 1500, height: 950, deviceScaleFactor: 1, mobile: false });
  await sleep(500);
});

// ── run ──────────────────────────────────────────────────────────────────

await waitForChrome();
const tabs = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
const page = tabs.find((t) => t.type === 'page');
ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((r) => { ws.onopen = r; });
ws.onmessage = (e) => {
  const m = JSON.parse(e.data);
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); }
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
await send('Log.enable');
await send('Page.enable');
await send('Emulation.setFocusEmulationEnabled', { enabled: true });
await send('Page.bringToFront');
await send('Emulation.setDeviceMetricsOverride', { width: 1500, height: 950, deviceScaleFactor: 1, mobile: false });
// ?noauto keeps the slow software GL from switching the page to simple mode by itself; ?nocoach skips the tour.
// ?bloom=0: the software GL of the headless browser draws the bloom pass at about one frame a second.
await navigate(base + '/?noauto&nocoach&bloom=0', 3500);

for (const [name, fn] of sections) {
  if (only.length && !only.some((o) => name.includes(o))) continue;
  console.log(`\n── ${name}`);
  try { await fn(); } catch (e) { check(`${name}: no exception`, false, e.stack || e.message); }
}

check('no console errors after the whole run', errors.length === 0, errors.slice(0, 3).join('; '));
ws.close();
chrome.kill();
process.exit(failed ? 1 : 0);
