#!/usr/bin/env node
// Checks of the 3D world in headless Chromium over the DevTools protocol.
// No dependencies (Node 22+ has WebSocket). With the server running:
//   FIXTURE=DIR node scripts/ui-check.mjs http://127.0.0.1:7117 [screenshot-dir]
// `scripts/run-checks.sh` starts the server on the world of scripts/fixture.mjs.
// The fixture's logs are the witness for every board command.

import { spawn } from 'node:child_process';
import { mkdtempSync, writeFileSync, readFileSync, existsSync, unlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { parseOptions } from '../web/requests.js';

const base = process.argv[2] || 'http://127.0.0.1:7117';
const shots = process.argv[3] || null;
const fixture = process.env.FIXTURE;
if (!fixture) { console.error('ui-check: FIXTURE is required'); process.exit(2); }
const port = 9333 + Math.floor(Math.random() * 400);
const profile = mkdtempSync(join(tmpdir(), 'herdr-planner-ui-'));
const chrome = spawn(process.env.CHROMIUM || 'chromium', [
  '--headless=new', '--no-sandbox', '--disable-gpu', '--hide-scrollbars', `--user-data-dir=${profile}`,
  `--remote-debugging-port=${port}`, '--window-size=1400,1000',
  '--host-resolver-rules=MAP remote.test 127.0.0.1', 'about:blank',
], { stdio: 'ignore' });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let failed = 0;
function check(name, ok, extra = '') {
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${name}${extra ? ' — ' + extra : ''}`);
  if (!ok) failed++;
}
check('plain bullets are not answer options', parseOptions('Сделал:\n- обновил README\n- поправил тесты\n\nВсё ок? Это всё.').length === 0);
check('bullets under a question are not options', parseOptions('Что делаем?\n- один\n- два').length === 0);
check('numbered options after the question are answers', JSON.stringify(parseOptions('Что делаем?\n1) **Да**\n2) `Нет`')) === JSON.stringify([{ label: '1)', value: 'Да' }, { label: '2)', value: 'Нет' }]));

async function waitForChrome() {
  for (let i = 0; i < 60; i++) {
    try { const r = await fetch(`http://127.0.0.1:${port}/json/version`); if (r.ok) return; } catch {}
    await sleep(200);
  }
  throw new Error('chromium did not start');
}

let ws; let id = 0; const pending = new Map(); const errors = [];
const send = (method, params = {}) => new Promise((r) => { const i = ++id; pending.set(i, r); ws.send(JSON.stringify({ id: i, method, params })); });
let lastNavAt = 0;
const navigate = async (url, ms) => {
  lastNavAt = Date.now();
  await send('Page.navigate', { url });
  await sleep(ms);
};
const keyText = async (ch) => {
  await send('Input.dispatchKeyEvent', { type: 'keyDown', text: ch, key: ch });
  await send('Input.dispatchKeyEvent', { type: 'keyUp', key: ch });
};
const evalJs = async (expr) => {
  const reply = await Promise.race([
    send('Runtime.evaluate', { expression: expr, returnByValue: true, awaitPromise: true }),
    sleep(8000).then(() => null),
  ]);
  if (!reply) { console.log('eval timeout:', expr.replace(/\s+/g, ' ').slice(0, 100)); return undefined; }
  return reply.result?.result?.value;
};
const text = (sel) => evalJs(`document.querySelector(${JSON.stringify(sel)})?.innerText || ''`);
const click = (sel) => evalJs(`(() => { const e = document.querySelector(${JSON.stringify(sel)}); if (e) e.click(); return !!e; })()`);
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
const calls = (file = 'commands.jsonl') => {
  const f = join(fixture, 'log', file);
  return existsSync(f) ? readFileSync(f, 'utf8').split('\n').filter(Boolean).map((l) => JSON.parse(l)) : [];
};
const lastCall = (file = 'commands.jsonl') => calls(file).at(-1);
const stateApi = () => evalJs('fetch("/api/state").then(r => r.json())');
const fly = async (target) => { await evalJs(`world.flyTo(${JSON.stringify(target)})`); await sleep(250); };
const debug = () => evalJs('window.world ? world.debug() : null');
const clearToasts = () => evalJs('document.querySelector("#toasts").replaceChildren(); true');
const LAB_STATS = `(() => {
  const els = [...document.querySelectorAll('.lbl3d[data-id^="alpha/"]')];
  const rects = els.map((e) => ({ id: e.dataset.id, cls: e.className, r: e.getBoundingClientRect(), p: world.screenOf(e.dataset.id) }));
  const chips = rects.filter((x) => x.cls.split(' ').includes('compact'));
  let overlaps = 0; const over = [];
  for (let i = 0; i < chips.length; i++) for (let j = i + 1; j < chips.length; j++) {
    const a = chips[i].r, b = chips[j].r;
    if (Math.min(a.right, b.right) - Math.max(a.left, b.left) > 2 && Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top) > 2) { overlaps++; over.push([chips[i].id, chips[j].id]); }
  }
  const below = rects.filter((x) => x.p && x.r.bottom > x.p.y + 2).map((x) => x.id);
  const full = rects.filter((x) => { const c = x.cls.split(' '); return !c.includes('compact') && !c.includes('dot'); }).length;
  return { count: rects.length, overlaps, over, below, full, camera: world.debug().camera, view: world.debug().view };
})()`;
const LIMITS_FIT = `(() => {
  const p = document.querySelector('.pop.limits');
  if (!p) return { ok: false };
  const r = p.getBoundingClientRect();
  const hit = (x, y) => { const h = document.elementFromPoint(x, y); return !!h && p.contains(h); };
  const inView = r.left >= 0 && r.top >= 0 && r.right <= innerWidth + 1 && r.bottom <= innerHeight + 1;
  const topHit = hit(r.left + r.width / 2, r.top + 8);
  const bottomHit = hit(r.left + r.width / 2, r.bottom - 8);
  const atBottom = document.elementFromPoint(r.left + r.width / 2, r.bottom - 8);
  return {
    ok: inView && topHit && bottomHit, inView, topHit, bottomHit,
    top: Math.round(r.top), bottom: Math.round(r.bottom), vh: innerHeight,
    bottomEl: atBottom ? atBottom.tagName + '.' + atBottom.className : null,
    focusHidden: document.querySelector('#focus').hidden,
    rect: [Math.round(r.left), Math.round(r.top), Math.round(r.width), Math.round(r.height)],
  };
})()`;
async function waitLog(pred, ms = 8000) {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (pred()) return true; await sleep(120); }
  return false;
}

await waitForChrome();
const tabs = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
const page = tabs.find((t) => t.type === 'page');
ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((r) => { ws.onopen = r; });
ws.onmessage = (e) => {
  const m = JSON.parse(e.data);
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); }
  else if (m.method === 'Runtime.exceptionThrown') {
    // An evaluated fetch can be cut off by the page navigation that follows a
    // check; that abort is harness noise, not a page error. Keep every other
    // failure, and only forgive it for a short window around a navigation.
    const text = m.params.exceptionDetails?.exception?.description || m.params.exceptionDetails?.text || 'exception';
    const navigationAbort = String(text).includes('Failed to fetch') && Date.now() - lastNavAt < 4000;
    if (!navigationAbort) errors.push(text);
  }
  else if (m.method === 'Runtime.consoleAPICalled' && m.params.type === 'error') errors.push(m.params.args?.map((a) => a.value || a.description).join(' '));
  else if (m.method === 'Log.entryAdded' && m.params.entry.level === 'error') {
    // A 502 from a refused board command is the expected answer to a bad link;
    // the page shows the board's words in a toast, not a JavaScript error.
    if (!String(m.params.entry.text).includes('Failed to load resource')) errors.push(m.params.entry.text);
  }
};
await send('Runtime.enable');
await send('Log.enable');
await send('Page.enable');
await navigate(base + '/', 2200);
check('the world starts', await waitFor('window.world && world.debug().planets > 0', 12000));
check('the page has a title and no console errors', (await evalJs('document.title')).includes('Herdr Planner') && errors.length === 0, errors.slice(0, 2).join('; '));
check('the world fills the viewport', await evalJs('Math.abs(document.querySelector("#win").getBoundingClientRect().width - innerWidth) < 2'));

const d0 = await debug();
check('WebGL is on', d0?.webgl === true, JSON.stringify(d0 && { simple: d0.simple, fps: d0.fps }));
check('every active task is a planet', d0?.planets === 17, String(d0?.planets));
check('the seven active statuses are drawn', ['failed', 'plan', 'queued', 'ready', 'review', 'waiting', 'working'].every((s) => d0?.statuses.includes(s)), (d0?.statuses || []).join(','));
check('universes are projects, quiet ones marked', d0?.universes === 5 && d0?.quiet === 1, JSON.stringify({ u: d0?.universes, q: d0?.quiet }));
check('the board dependencies are links', d0?.links.some((l) => l.from === 'alpha/9' && l.to === 'alpha/10') && d0?.links.some((l) => l.from === 'alpha/4' && l.to === 'gamma/1'), JSON.stringify(d0?.links));
check('the HUD counts requests and universes', /ЖДУТ ТЕБЯ\s+11/i.test(await text('#hud')) && /ВСЕЛЕННЫХ\s+4/i.test(await text('#hud')) && /ТИХИХ\s+1/i.test(await text('#hud')), (await text('#hud')).replace(/\s+/g, ' ').slice(0, 240));
await shot('world-overview.png');

// Subscription limits: Omarchy's records are in the fixture, the HUD chip
// names the tightest window, the world draws one arc per agent with limits,
// and the panel keeps every window with its reset and freshness.
check('the HUD has a limits chip', await waitFor('!!document.querySelector("#hud-limits")', 8000), await text('#hud-limits'));
const limitsDebug = await debug();
check('one world arc per agent with limits', limitsDebug?.limitArcs === 1, JSON.stringify(limitsDebug && { arcs: limitsDebug.limitArcs, crit: limitsDebug.limitCrit }));
check('a 95%+ window makes the halo critical', limitsDebug?.limitCrit === true, JSON.stringify(limitsDebug && { crit: limitsDebug.limitCrit }));
const chipText = (await text('#hud-limits')).replace(/\s+/g, ' ');
check('the chip names the worst window and its remainder', /3\s*%/.test(chipText) && /Fable Weekly/.test(chipText), chipText);
await clearToasts();
await click('#hud-limits');
check('the limits panel opens', await waitFor('!!document.querySelector(".pop.limits")', 3000));
// The panel must be seen, not merely present: Focus is open on load and the
// HUD used to be a stacking context that kept the popover underneath it.
const limitsBox = await evalJs(`(() => {
  const p = document.querySelector('.pop.limits');
  if (!p) return { ok: false };
  const r = p.getBoundingClientRect();
  const hit = (x, y) => { const h = document.elementFromPoint(x, y); return !!h && p.contains(h); };
  const x = r.left + r.width / 2, y = r.top + r.height / 2;
  const chip = document.querySelector('#hud-limits');
  const cr = chip.getBoundingClientRect();
  const chipHit = document.elementFromPoint(cr.left + cr.width / 2, cr.top + cr.height / 2);
  const inView = r.left >= 0 && r.top >= 0 && r.right <= innerWidth + 1 && r.bottom <= innerHeight + 1;
  return {
    inView, top: hit(x, r.top + 8), center: hit(x, y), bottom: hit(r.right - 8, r.bottom - 8),
    chip: !!chipHit && (chipHit === chip || chip.contains(chipHit)),
    rect: [Math.round(r.left), Math.round(r.top), Math.round(r.width), Math.round(r.height)],
    focusOpen: !document.querySelector('#focus').hidden,
  };
})()`);
check('the limits panel is above Focus and fully in view', limitsBox.inView && limitsBox.top && limitsBox.center && limitsBox.bottom, JSON.stringify(limitsBox));
check('the open limits panel does not cover its own chip', limitsBox.chip === true, JSON.stringify(limitsBox));
const limitsText = (await text('.pop.limits')).replace(/\s+/g, ' ');
check('the panel lists Claude, omp and pi', /Claude/.test(limitsText) && /OMP/.test(limitsText) && /Pi/.test(limitsText), limitsText.slice(0, 240));
check('remaining is the inverse of the used share', /осталось 79 %/.test(limitsText) && /осталось 9 %/.test(limitsText) && /осталось 3 %/.test(limitsText), limitsText.slice(0, 260));
check('reset shows local time and the hours left', /сброс (?:\d\d\.\d\d, )?\d\d:\d\d · через \d+ ч/.test(limitsText), limitsText.slice(0, 260));
check('a later reset carries its date', /сброс \d\d\.\d\d, \d\d:\d\d/.test(limitsText), limitsText.slice(0, 260));
check('80% is a warning and 95% is critical', (await evalJs('document.querySelectorAll(".lim-win.warn").length')) === 1 && (await evalJs('document.querySelectorAll(".lim-win.crit").length')) === 1);
check('omp and pi honestly say there is no data', (await text('.lim-agent[data-agent="omp"]')).includes('нет данных') && (await text('.lim-agent[data-agent="pi"]')).includes('нет данных') && limitsText.includes('API keys · no subscription'));
check('an old record is marked stale', (await evalJs('document.querySelectorAll(".lim-agent.stale").length')) === 1 && limitsText.toLowerCase().includes('устарело'));
check('every agent reports when its numbers were refreshed', (limitsText.match(/обновлено /g) || []).length === 3, limitsText.slice(0, 260));
await shot('world-limits.png');
const limitsAlert = await api('/api/limits');
check('the bar alert names the worst live window', limitsAlert[0] === 200 && limitsAlert[1].level === 'crit' && limitsAlert[1].worst.label === 'Fable Weekly' && limitsAlert[1].worst.remaining_percent === 3, JSON.stringify(limitsAlert[1]));
await keyText('q');
check('Q closes the limits panel', await waitFor('!document.querySelector(".pop.limits")', 3000));

// The God's todo: a list near the chat; a drag into a universe makes a planet.
check('the God has a todo list near the chat', await evalJs('document.querySelectorAll("#todo-list .todo-item").length === 2'), await evalJs('document.querySelectorAll("#todo-list .todo-item").length'));
await evalJs('document.querySelector("#todo-in").value = "Проверить деплой"; document.querySelector("#todo-new").requestSubmit()');
check('a new todo item is kept', await waitFor('document.querySelectorAll("#todo-list .todo-item").length === 3', 6000));
const todoFile = () => JSON.parse(readFileSync(join(fixture, 'state', 'herdr-planner', 'todo.json'), 'utf8'));
check('the todo list is on the server', todoFile().some((x) => x.text === 'Проверить деплой'));
const rowsBefore = (await stateApi()).tasks.length + (await stateApi()).archived.length;
await evalJs(`(() => {
  const lab = document.querySelector('.lbl3d.uni[data-universe="alpha"]');
  const dt = new DataTransfer(); dt.setData('text/plain', 't1');
  lab.dispatchEvent(new DragEvent('dragover', { bubbles: true, cancelable: true, dataTransfer: dt }));
  lab.dispatchEvent(new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer: dt }));
  return true;
})()`);
check('dropping a todo into a universe creates a task', await waitFor(`fetch('/api/state').then(r=>r.json()).then(s => s.tasks.some(t => t.repo === 'alpha' && t.title === 'Починить кэш отчётов'))`, 15000), JSON.stringify(lastCall()));
check('the board received add with the universe', calls().some((c) => c.argv[0] === 'add' && c.argv.includes('--repo') && c.argv.includes('alpha')), JSON.stringify(lastCall()));
check('the created task is a new planet', await waitFor(`(${JSON.stringify(false)}) || fetch('/api/state').then(r=>r.json()).then(s => s.tasks.some(t => t.repo === 'alpha' && t.title === 'Починить кэш отчётов')) && world.debug().planets === 18`, 15000), String((await debug())?.planets));
check('the dropped item leaves the todo list', await evalJs('document.querySelectorAll("#todo-list .todo-item").length === 2') && !todoFile().some((x) => x.id === 't1'));
const rowsAfter = (await stateApi()).tasks.length + (await stateApi()).archived.length;
check('the board grew by one task', rowsAfter === rowsBefore + 1, `${rowsBefore} -> ${rowsAfter}`);

// Editing and removing an item.
await evalJs(`(() => { const s = document.querySelector('[data-todo-text="t2"]'); s.dispatchEvent(new MouseEvent('dblclick', { bubbles: true })); return true; })()`);
await evalJs(`(() => { const i = document.querySelector('.todo-item .edit'); if (!i) return false; i.value = 'Тёмная тема и настройки'; i.dispatchEvent(new Event('input', { bubbles: true })); return true; })()`);
await sleep(5200);
check('a todo editor survives the 4-second refresh', await evalJs('!!document.querySelector(".todo-item .edit") && document.activeElement.classList.contains("edit")'), JSON.stringify({ input: await evalJs('!!document.querySelector(".todo-item .edit")'), active: await evalJs('document.activeElement.className') }));
await evalJs(`(() => { const i = document.querySelector('.todo-item .edit'); if (!i) return false; i.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })); return true; })()`);
let editedTodo = false;
for (let i = 0; i < 40 && !editedTodo; i++) { if (todoFile().some((x) => x.text === 'Тёмная тема и настройки')) editedTodo = true; else await sleep(150); }
check('a todo item can be edited', editedTodo, JSON.stringify(todoFile()));
await evalJs('document.querySelector("[data-act=\"todo-del\"][data-todo-id=\"t2\"]").click()');
await waitFor('document.querySelector(\'[data-todo-text="t2"]\')?.textContent.includes("Тёмная тема и настройки")', 4000);
const removedTodo = await evalJs('(() => { const b = document.querySelector(\'[data-act="todo-del"][data-todo-id="t2"]\'); if (!b) return false; b.click(); return true; })()');
check('a todo item can be removed', removedTodo && await waitFor('document.querySelectorAll("#todo-list .todo-item").length === 1', 5000), JSON.stringify({ removedTodo, n: await evalJs('document.querySelectorAll("#todo-list .todo-item").length'), todo: todoFile() }));

// Labels: the full label of a planet carries the key and the question itself.
await fly('alpha/2');
const labelOk = await waitFor(`(() => {
  const e = document.querySelector('.lbl3d[data-id="alpha/2"]');
  return !!e && e.textContent.includes('ALP-2') && e.textContent.includes('Какую схему брать');
})()`, 7000);
check('a planet label carries its key and question', labelOk, JSON.stringify({ ids: await evalJs('[...document.querySelectorAll(".lbl3d[data-id]")].map(e=>e.dataset.id).slice(0,30)'), d: await debug() }));

// A text answer survives the 4-second refresh, and the rest of the typing is
// still text: a lost focus would turn 'д' into link mode.
await fly('alpha/3');
await click('#focus [data-act="answer"]');
check('"return with remarks" focuses the answer field', await waitFor('document.activeElement.id === "tell"', 4000), await evalJs('document.activeElement.id'));
await send('Input.insertText', { text: 'верни пла' });
await sleep(5200);
check('the answer field keeps focus across a refresh', await evalJs('document.activeElement.id === "tell"'), await evalJs('document.activeElement.tagName + "/" + (document.activeElement.id || "")'));
await keyText('д');
check('typing after a refresh stays in the field', await evalJs('document.querySelector("#tell").value.endsWith("д")'), await evalJs('document.querySelector("#tell").value'));
check('a lost letter does not turn on link mode', await evalJs('plannerDebug().linkMode === false'));
check('a lost letter opens no drawers and keeps Focus', await evalJs('document.querySelector("#drawer-log").hidden && document.querySelector("#drawer-wait").hidden && document.querySelector("#drawer-nav").hidden && !document.querySelector("#focus").hidden'));
await evalJs('(() => { const i = document.querySelector("#tell"); i.value = ""; i.dispatchEvent(new Event("input", { bubbles: true })); return true; })()');
// Fly to a working task: focus shows title, worktree and branch.
await fly('alpha/6');
check('a flight opens the focus panel with details', await waitFor('document.querySelector("#focus").innerText.includes("Миграция базы") && document.querySelector("#focus").innerText.includes("worktrees") && document.querySelector("#focus").innerText.includes("work/6")', 8000), (await text('#focus')).replace(/\s+/g, ' ').slice(0, 200));
check('focus offers the agent message field', await evalJs('!!document.querySelector("#tell")'));
await shot('world-focus.png');
await evalJs('document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }))');
check('Esc returns to the overview', await waitFor('world.debug().view === "overview"', 3000), await evalJs('world.debug().view'));

// A confirmation belongs to one task and one command.
await fly('alpha/4');
await click('#focus [data-act="approve"]');
check('approve arms visibly', (await text('#focus [data-act="approve"]')).includes('Подтвердить'));
await click('#focus [data-act="anyway"]');
check('anyway does not reuse the approve arm', !calls().some((c) => c.argv[0] === 'approve' && c.argv[1] === 'alpha/4'), JSON.stringify(lastCall()));
check('anyway has its own visible confirmation', (await text('#focus [data-act="anyway"]')).includes('Подтвердить «всё равно»'));

const bare = await fetch(base + '/api/action', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ action: 'approve', id: 'alpha/3' }) });
check('the API refuses an approval without confirmation', bare.status === 400, String(bare.status));
const bareAnyway = await fetch(base + '/api/action', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ action: 'anyway', id: 'alpha/5' }) });
check('the API refuses anyway without confirmation', bareAnyway.status === 400, String(bareAnyway.status));
const badId = await fetch(base + '/api/action', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ action: 'retry', id: '--all/1' }) });
check('a malformed task id is a 400, not a gateway error', badId.status === 400, String(badId.status));
await fly('alpha/4');
await click('#focus [data-act="approve"]');
check('approve arms on the first click', await waitFor('document.querySelector(\'#focus [data-act="approve"]\')?.textContent.includes("Подтвердить")'));
await sleep(700);
await click('#focus [data-act="approve"]');
check('the second click runs herdr-projects approve', await waitFor('fetch("/api/state").then(r=>r.json()).then(s => s.tasks.find(t => t.id === "alpha/4")?.status === "done")', 12000));
check('the board received approve', calls().some((c) => c.argv[0] === 'approve' && c.argv[1] === 'alpha/4'), JSON.stringify(lastCall()));

// «Всё равно влить» shows its confirmation; a double click does not confirm it.
await fly('alpha/5');
await click('#focus [data-act="anyway"]');
check('anyway arms on the first click', (await text('#focus [data-act="anyway"]')).includes('Подтвердить «всё равно»'));
const anywayBefore = calls().filter((c) => c.argv[0] === 'approve' && c.argv[1] === 'alpha/5').length;
await evalJs('document.querySelector("#focus [data-act=\\"anyway\\"]").click(); document.querySelector("#focus [data-act=\\"anyway\\"]").click(); true');
await sleep(120);
check('a double-click does not send anyway', calls().filter((c) => c.argv[0] === 'approve' && c.argv[1] === 'alpha/5').length === anywayBefore, JSON.stringify(lastCall()));
await sleep(700);
await click('#focus [data-act="anyway"]');
check('the confirmed click runs approve --anyway', await waitFor('fetch("/api/state").then(r=>r.json()).then(s => s.tasks.find(t => t.id === "alpha/5")?.status === "done")', 12000));
check('the board received approve --anyway', calls().some((c) => c.argv[0] === 'approve' && c.argv[1] === 'alpha/5' && c.argv[2] === '--anyway'), JSON.stringify(lastCall()));

// Focus actions: fix/finish, retry and a message to the agent.
await fly('alpha/8');
await click('#focus [data-act="retry"]');
check('retry reaches the board', await waitFor('fetch("/api/state").then(r=>r.json()).then(s => s.tasks.find(t => t.id === "alpha/8")?.status === "working")', 12000));
await fly('alpha/2');
await evalJs('document.querySelector("#tell").value = "старую"; document.querySelector("[data-act=\\"tell-agent\\"]").click()');
const told = (t) => calls().some((c) => c.argv[0] === 'tell' && c.argv[1] === 'alpha/2' && c.argv[2] === '--' && c.argv[3] === t);
for (let i = 0; i < 60 && !told('старую'); i++) await sleep(150);
check('a message to the agent reaches herdr-projects tell', told('старую'), JSON.stringify(lastCall()));
const dashTell = await api('/api/action', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ action: 'tell', id: 'alpha/2', text: '-- hi' }) });
check('a message starting with -- is passed as text, not an option', dashTell[0] === 200 && told('-- hi'), JSON.stringify(lastCall()));

// A permission request: the prepared text is editable and goes through tell.
await fly('alpha/16');
check('a permission request shows its full text', (await text('#focus')).includes('удалить девять старых веток'), (await text('#focus')).slice(0, 80));
await click('#focus [data-act="perm-allow"]');
check('a permission button prepares an editable answer', (await evalJs('document.querySelector("#tell")?.value')) === 'Да, разрешаю.');
await evalJs('document.querySelector("[data-act=\\"tell-agent\\"]").click()');
for (let i = 0; i < 40 && !calls().some((c) => c.argv[0] === 'tell' && c.argv[1] === 'alpha/16' && c.argv[3] === 'Да, разрешаю.'); i++) await sleep(150);
check('the prepared permission answer reaches the board', calls().some((c) => c.argv[0] === 'tell' && c.argv[1] === 'alpha/16' && c.argv[3] === 'Да, разрешаю.'), JSON.stringify(lastCall()));

// Pick one attempt.
await fly('delta/3');
check('a pick request lists the attempts', await waitFor('document.querySelectorAll("#focus .pick").length === 3', 8000), await evalJs('document.querySelectorAll("#focus .pick").length'));
await click('#focus [data-act="pick"][data-letter="B"]');
for (let i = 0; i < 50 && !calls().some((c) => c.argv[0] === 'pick' && c.argv[1] === 'delta/3' && c.argv[2] === 'B'); i++) await sleep(150);
check('choosing an attempt reaches herdr-projects pick', calls().some((c) => c.argv[0] === 'pick' && c.argv[1] === 'delta/3' && c.argv[2] === 'B'), JSON.stringify(lastCall()));
check('the picked task leaves the request list', await waitFor('fetch("/api/state").then(r=>r.json()).then(s => !s.tasks.some(t => t.id === "delta/3" && t.status === "waiting"))', 12000));

// Fix and finish are real board commands.
await fly('sigma/1');
await click('#focus [data-act="fix"]');
check('fix reaches the board', await waitFor('fetch("/api/state").then(r=>r.json()).then(s => s.tasks.find(t => t.id === "sigma/1")?.status === "working")', 12000));
await api('/api/action', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ action: 'finish', id: 'sigma/1' }) });
check('finish reaches the board', await waitFor('fetch("/api/state").then(r=>r.json()).then(s => s.tasks.find(t => t.id === "sigma/1")?.status === "done")', 12000));
check('the board logged fix and finish', calls().some((c) => c.argv[0] === 'fix') && calls().some((c) => c.argv[0] === 'finish'), JSON.stringify(calls().slice(-2)));

// Links: draw, refuse a circle, refuse an unknown task, remove, undo.
await evalJs('world.connect("alpha/11", "alpha/6")');
check('a drawn link reaches herdr-projects after', await waitFor(`fetch('/api/state').then(r=>r.json()).then(s => s.links.some(l => l.from === 'alpha/6' && l.to === 'alpha/11'))`, 12000), JSON.stringify((await debug())?.links));

// A wait on an archived task keeps its id; later link edits must not send its tag.
await evalJs('world.connect("alpha/11", "gamma/2")');
check('a wait on an archived task is drawn as a far star', await waitFor(`fetch('/api/state').then(r=>r.json()).then(s => s.links.some(l => l.from === 'gamma/2' && l.to === 'alpha/11' && l.kind === 'far' && l.label === 'GAM-2'))`, 12000), JSON.stringify((await debug())?.links));
await evalJs('world.connect("alpha/11", "alpha/9")');
check('a later link edit keeps the archived wait', await waitFor(`fetch('/api/state').then(r=>r.json()).then(s => s.links.some(l => l.from === 'alpha/9' && l.to === 'alpha/11') && s.links.some(l => l.from === 'gamma/2' && l.to === 'alpha/11'))`, 12000), JSON.stringify((await debug())?.links));
check('the after call carries the archived id, not its tag', await waitLog(() => calls().some((c) => c.argv[0] === 'after' && c.argv[1] === 'alpha/11' && String(c.argv[2]).includes('gamma/2')), 8000), JSON.stringify(lastCall()));
check('no bad task reference for the archived wait', !(await evalJs('[...document.querySelectorAll(".toast")].some(t => t.innerText.includes("bad task reference"))')));

const circleBefore = calls().filter((c) => c.argv[0] === 'after').length;
await evalJs('world.connect("alpha/9", "alpha/10")');
check('a circle is refused with the board words', await waitFor('[...document.querySelectorAll(".toast")].some(t => t.innerText.includes("wait on each other"))', 8000), await text('.toast'));
check('a refused circle writes no after', calls().filter((c) => c.argv[0] === 'after').length === circleBefore, JSON.stringify(lastCall()));
await evalJs('world.connect("alpha/11", "zeta/9")');
check('an unknown task is refused', await waitFor('[...document.querySelectorAll(".toast")].some(t => t.innerText.includes("No task zeta/9"))', 8000), await evalJs('[...document.querySelectorAll(".toast")].map(t => t.innerText).join(" | ")'));
check('an unknown task writes no after', calls().filter((c) => c.argv[0] === 'after').length === circleBefore, JSON.stringify(lastCall()));
await fly('alpha/11');
check('focus lists the link and offers removal', await waitFor('!!document.querySelector(\'#focus [data-act="after-remove"][data-from="alpha/6"]\')', 5000));
await click('#focus [data-act="after-remove"][data-from="alpha/6"]');
check('removing a link clears that wait, keeps the others', await waitFor(`fetch('/api/state').then(r=>r.json()).then(s => !s.links.some(l => l.from === 'alpha/6' && l.to === 'alpha/11') && s.links.some(l => l.from === 'gamma/2' && l.to === 'alpha/11') && s.links.some(l => l.from === 'alpha/9' && l.to === 'alpha/11'))`, 12000), JSON.stringify((await debug())?.links));
check('the board received after with the remaining waits', await waitLog(() => calls().some((c) => c.argv[0] === 'after' && c.argv[1] === 'alpha/11' && String(c.argv[2]).includes('gamma/2') && !String(c.argv[2]).includes('alpha/6')), 8000), JSON.stringify(lastCall()));
// Target the removal toast by its text: other toasts (a refused circle, an
// unknown task) can still be on screen, and an undo must not be lost because
// the last node happened to be one of them.
// The fixture writes its log when the command starts, so the board call can be
// visible before the browser has the server's answer and the toast; wait for
// the toast itself instead of racing it.
await waitFor(`[...document.querySelectorAll(".toast")].some((n) => n.innerText.startsWith("ALP-11: начнётся после GAM-2, ALP-9") && n.querySelector(".undo"))`, 12000);
const undoState = await evalJs(`(() => {
  const all = [...document.querySelectorAll(".toast")].map((n) => n.innerText.replace(/\\n/g, ' | '));
  const t = [...document.querySelectorAll(".toast")].reverse().find((n) => n.innerText.startsWith("ALP-11: начнётся после GAM-2, ALP-9") && n.querySelector(".undo"));
  const b = t && t.querySelector(".undo");
  if (b) b.click();
  return { clicked: !!b, toasts: all };
})()`);
check('the removal toast has an undo', undoState?.clicked === true, JSON.stringify(undoState?.toasts));
check('undo restores the previous wait', await waitFor(`fetch('/api/state').then(r=>r.json()).then(s => s.links.some(l => l.from === 'alpha/6' && l.to === 'alpha/11'))`, 12000), JSON.stringify((await debug())?.links));

// Coordinator: options from its words, then a dialog on its screen.
check('the Coordinator offers answer options', await waitFor('document.querySelectorAll("#cvariants .btn").length === 2', 6000), await evalJs('document.querySelectorAll("#cvariants .btn").length'));
await click('#cvariants .btn:nth-child(2)');
check('an option is sent as coordinator tell', await waitLog(() => calls('coordinator.jsonl').some((c) => c.text === 'Разобрать «Ждёт тебя»'), 8000), JSON.stringify(lastCall('coordinator.jsonl')));

// `/` opens the chat: the last three turns of the Coordinator's transcript above the line;
// Up (or the wheel) pulls earlier ones in.
const press = async (key, code) => {
  await send('Input.dispatchKeyEvent', { type: 'rawKeyDown', key, code: key, windowsVirtualKeyCode: code });
  await send('Input.dispatchKeyEvent', { type: 'keyUp', key, code: key, windowsVirtualKeyCode: code });
};
await evalJs('document.activeElement && document.activeElement.blur()');
await keyText('/');
check('/ focuses the line and opens the chat', await waitFor('document.activeElement.id === "ask-in" && document.querySelector("#console").classList.contains("open")', 6000));
check('the open chat shows the last three turns', await waitFor('document.querySelectorAll("#clog .turn:not(.pending)").length === 3', 6000), await evalJs('document.querySelectorAll("#clog .turn:not(.pending)").length'));
check('the newest turn is at the bottom, its answer still pending', (await evalJs('(() => { const t = [...document.querySelectorAll("#clog .turn:not(.pending)")]; return t[t.length - 1].innerText; })()')).includes('Вопрос 10') && (await evalJs('(() => { const l = document.querySelector("#clog"); return l.scrollHeight - l.scrollTop - l.clientHeight < 30; })()')));
check('the line stayed empty after /', (await evalJs('document.querySelector("#ask-in").value')) === '');
await sleep(1800);
check('the open chat is docked and fully in view', await evalJs('(() => { const r = document.querySelector("#console").getBoundingClientRect(); return r.left >= 0 && r.top >= 0 && r.right <= innerWidth && r.bottom <= innerHeight; })()'), await evalJs('JSON.stringify(document.querySelector("#console").getBoundingClientRect())'));
await shot('chat-open.png');
for (let i = 0; i < 12; i++) await press('ArrowUp', 38);
check('Up pulls earlier turns in', await waitFor('document.querySelectorAll("#clog .turn:not(.pending)").length === 8', 6000), await evalJs('document.querySelectorAll("#clog .turn:not(.pending)").length'));
for (let i = 0; i < 60 && (await evalJs('document.querySelectorAll("#clog .turn:not(.pending)").length')) < 10; i++) { await press('ArrowUp', 38); await sleep(60); }
check('the oldest turns come last and the start is marked', await waitFor('document.querySelectorAll("#clog .turn:not(.pending)").length === 10 && !!document.querySelector("#clog .meta.start")', 6000), await evalJs('document.querySelectorAll("#clog .turn:not(.pending)").length'));
check('the answers are the Coordinator\'s words, tools left out', (await text('#clog')).includes('Ответ 1: ALP-1 идёт по плану.') && !(await text('#clog')).includes('tool_result'));
await evalJs('document.querySelector("#clog .you").click()');
check('a click on an old question returns it to the line', (await evalJs('document.querySelector("#ask-in").value')) === 'Вопрос 1: как дела по ALP-1?');
await evalJs('document.querySelector("#ask-in").value = ""; document.querySelector("#ask-in").dispatchEvent(new Event("input"))');
await evalJs('(() => { const t = document.querySelector("#ask-in"); t.value = "НОВЫЙ ВОПРОС"; t.dispatchEvent(new Event("input")); t.focus(); })()');
await press('Enter', 13);
check('a sent message shows in the open chat', await waitFor('document.querySelector("#clog").innerText.includes("НОВЫЙ ВОПРОС")', 4000));
await sleep(4500);
check('it stays there until the transcript has it', await evalJs('document.querySelector("#clog").innerText.includes("НОВЫЙ ВОПРОС") && document.querySelector("#console").classList.contains("open")'), await text('#clog'));
await press('Escape', 27);
check('Escape closes the chat', await waitFor('!document.querySelector("#console").classList.contains("open")', 4000));
check('the chat history answers with its page size', await evalJs(`fetch('/api/chat/history?limit=2').then(r=>r.json()).then(h => h.available && h.turns.length === 2 && h.turns[1].n === 9 && h.more)`));

writeFileSync(join(fixture, 'state', 'coordinator-dialog'), '1');
writeFileSync(join(fixture, 'state', 'dialog.txt'), '❯ May I remove the old branches?\n  1. Yes\n  2. No');
check('a Coordinator dialog is shown in the chat', await waitFor('!document.querySelector("#cdialog").hidden && document.querySelector("#cdialog").innerText.includes("May I remove the old branches")', 12000), await text('#cdialog'));
await click('#cdialog [data-act="keys"][data-keys="enter"]');
check('a key reaches herdr agent send-keys', await waitLog(() => calls('keys.jsonl').some((c) => c.target === 'sw-coordinator' && c.keys.includes('enter')), 8000), JSON.stringify(lastCall('keys.jsonl')));
// The reviewer's pane is '<agent>-r', not the worker's.
await fly('sigma/2');
check('a reviewneeds request offers the reviewer screen', await waitFor('!!document.querySelector(\'#focus [data-act="screen"][data-role="reviewer"]\')', 6000));
await click('#focus [data-act="screen"][data-role="reviewer"]');
check('the reviewer screen is read from the -r agent', await waitLog(() => calls('herdr.jsonl').some((c) => c.argv[0] === 'agent' && c.argv[1] === 'read' && c.argv[2] === 'sw-sigma-2-r'), 8000), JSON.stringify(calls('herdr.jsonl').slice(-2)));
check('reviewer keys go to the -r agent', await waitFor('!!document.querySelector(\'#focus [data-act="keys"][data-keys="enter"][data-role="reviewer"]\')', 5000));
await click('#focus [data-act="keys"][data-keys="enter"][data-role="reviewer"]');
check('reviewer send-keys names the -r agent', await waitLog(() => calls('keys.jsonl').some((c) => c.target === 'sw-sigma-2-r' && c.keys.includes('enter')), 8000), JSON.stringify(calls('keys.jsonl').slice(-2)));
unlinkSync(join(fixture, 'state', 'coordinator-dialog'));

// Yazi is local; a non-loopback page is remote and only shows the path.
await fly('alpha/6');
await click('#focus [data-act="files"]');
for (let i = 0; i < 25 && !calls('terminal.jsonl').some((c) => c.argv[0] === 'yazi' && c.argv[1].includes('worktrees')); i++) await sleep(150);
check('the folder button opens yazi in the worktree', calls('terminal.jsonl').some((c) => c.argv[0] === 'yazi' && c.argv[1].includes('worktrees')), JSON.stringify(lastCall('terminal.jsonl')));

const remoteURL = `${new URL(base).protocol}//remote.test:${new URL(base).port}/`;
const terminalsBefore = calls('terminal.jsonl').length;
await navigate(remoteURL, 2200);
check('a non-loopback page is a remote client', await waitFor('window.plannerDebug && plannerDebug().local === false', 5000), JSON.stringify(await evalJs('window.plannerDebug && plannerDebug()')));
check('a remote page still shows the limits chip', await waitFor('document.querySelector("#hud-limits")?.innerText.includes("3%")', 8000), await text('#hud-limits'));
await fly('alpha/6');
await click('#focus [data-act="files"]');
check('a remote file button shows the path', await waitFor('document.querySelector(".toast")?.innerText.includes("worktrees")', 5000), await text('.toast'));
check('a remote file button does not start yazi', calls('terminal.jsonl').length === terminalsBefore);
await fly('alpha/2');
await evalJs('document.querySelector("#tell").value = "удалённый ответ"; document.querySelector("[data-act=\\"tell-agent\\"]").click()');
check('a remote client answers a question', await waitLog(() => calls().some((c) => c.argv[0] === 'tell' && c.argv[1] === 'alpha/2' && c.argv[3] === 'удалённый ответ'), 8000), JSON.stringify(lastCall()));
await navigate(base + '/', 800);
check('the world is back after the remote visit', await waitFor('window.world && world.debug().planets > 0 && world.debug().labels > 0', 20000), JSON.stringify({ d: await debug(), labels: await evalJs('document.querySelectorAll(".lbl3d").length') }));

// Universe view: keys above planets, not full labels over them.
await evalJs('world.flyTo("alpha"); world.select(null); true');
await sleep(1500);
const lab = await evalJs(LAB_STATS);
check('a universe view shows compact planet labels', lab && lab.count > 5 && lab.full <= 1, JSON.stringify(lab));
check('planet labels do not overlap each other', lab && lab.overlaps === 0, JSON.stringify(lab));
check('planet labels sit above their planets', lab && lab.below.length === 0, JSON.stringify(lab));

// Zooming in part-way must not turn every nearby planet into a full label.
await evalJs('world.zoomBy(0.6); world.select(null); true');
await sleep(1000);
const zoomed = await evalJs(LAB_STATS);
check('zooming into a universe keeps full labels to one', zoomed && zoomed.full <= 1, JSON.stringify(zoomed));
check('zoomed labels do not overlap', zoomed && zoomed.overlaps === 0, JSON.stringify(zoomed));
check('zoomed labels stay above their planets', zoomed && zoomed.below.length === 0, JSON.stringify(zoomed));
await evalJs('world.orbitBy(1.1, 0.4); true');
await sleep(1000);
const low = await evalJs(LAB_STATS);
check('a low-angle view keeps full labels to one', low && low.full <= 1, JSON.stringify(low));
check('a low-angle view does not pile labels', low && low.overlaps === 0, JSON.stringify(low));
check('a low-angle view leaves planets visible', low && low.below.length === 0, JSON.stringify(low));

// Filters, archive and restore.
check('a stage filter button exists', await click('[data-stage="review"]'));
check('a stage filter dims the rest', await waitFor('world.debug().dim > 0 && document.querySelectorAll(".lbl3d.dim").length > 0', 8000), JSON.stringify({ d: await debug(), dom: await evalJs('document.querySelectorAll(".lbl3d.dim").length') }));
await click('[data-stage="review"]');
// Chat: the fake Coordinator answers.
await evalJs('document.querySelector("#ask-in").value = "проверка чата"; document.querySelector("#ask").requestSubmit()');
check('the chat sends coordinator tell', await waitFor(`fetch('/api/state').then(r => r.json()).then(s => s.coordinator.last_sent === 'проверка чата')`, 8000));
check('the Coordinator answer is shown in the console', await waitFor('document.querySelector("#clog").innerText.includes("Принял")', 10000));
const helpChat = await api('/api/chat', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ text: '--help' }) });
check('a Coordinator message starting with -- is literal', helpChat[0] === 200 && calls('coordinator.jsonl').some((c) => c.text === '--help'), JSON.stringify(lastCall('coordinator.jsonl')));

// Images: paste, drop and the file picker all upload through HTTP; previews
// can be removed; one message carries several images; the history shows them.
const pngB64 = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAFgwJ/lK3Q6wAAAABJRU5ErkJggg==';
await evalJs(`window.__shot = (name) => new File([Uint8Array.from(atob('${pngB64}'), (c) => c.charCodeAt(0))], name, { type: 'image/png' }); true`);
check('the chat has an attach button', await evalJs('!!document.querySelector("#attach-btn") && !!document.querySelector("#file-in")?.multiple'));
await evalJs(`(() => { const dt = new DataTransfer(); dt.items.add(__shot('paste.png')); const e = new ClipboardEvent('paste', { clipboardData: dt, bubbles: true, cancelable: true }); document.querySelector('#ask-in').dispatchEvent(e); return e.defaultPrevented; })()`);
check('pasting an image shows a preview', await waitFor('document.querySelectorAll("#attach .att").length === 1', 6000));
check('the pasted image is uploaded', await waitFor('document.querySelector("#attach .att")?.dataset.state === "ready"', 10000));
await evalJs(`(() => { const dt = new DataTransfer(); dt.items.add(__shot('drop.png')); document.querySelector('#console').dispatchEvent(new DragEvent('drop', { dataTransfer: dt, bubbles: true, cancelable: true })); return true; })()`);
check('dropping an image adds a second preview', await waitFor('document.querySelectorAll("#attach .att").length === 2', 6000));
await sleep(600);
await click('#attach .att .rm');
check('a preview can be removed before sending', await evalJs('document.querySelectorAll("#attach .att").length === 1'));
await evalJs(`(() => { const dt = new DataTransfer(); dt.items.add(__shot('picker.png')); const input = document.querySelector('#file-in'); input.files = dt.files; input.dispatchEvent(new Event('change', { bubbles: true })); return true; })()`);
check('the file picker adds another image', await waitFor('document.querySelectorAll("#attach .att").length === 2', 6000));
check('both images are ready', await waitFor('[...document.querySelectorAll("#attach .att")].every((a) => a.dataset.state === "ready")', 10000));
await evalJs('document.querySelector("#ask-in").value = "вот скриншоты"; document.querySelector("#ask").requestSubmit()');
check('the message and its images reach the Coordinator', await waitFor(`fetch('/api/state').then(r => r.json()).then(s => { const m = s.coordinator.history?.at(-1); return m?.text === 'вот скриншоты' && m.attachments?.length === 2; })`, 10000));
check('sent previews are cleared after sending', await waitFor('document.querySelectorAll("#attach .att").length === 0'));
check('the chat history shows thumbnails', await waitFor('document.querySelectorAll("#clog .shot img").length >= 2', 6000));
check('thumbnails load from the protected attachment route', await evalJs('[...document.querySelectorAll("#clog .shot img")].every((i) => i.getAttribute("src").startsWith("/api/attachments/"))'));
const imageTell = calls('coordinator.jsonl').at(-1);
const imagePaths = imageTell.text.split('\n').filter((l) => l.startsWith(join(fixture, 'state', 'herdr-planner', 'attachments') + '/'));
check('the Coordinator message carries both absolute image paths', imageTell.text.includes('вот скриншоты') && imagePaths.length === 2, imageTell.text.replace(/\n/g, ' | '));
check('the image files exist on this machine', imagePaths.length === 2 && imagePaths.every(existsSync));
check('the stored images are really PNG', imagePaths.every((p) => readFileSync(p).subarray(0, 8).equals(Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]))));

// A long history must open at its end: the newest message and the reply are
// what the chat is for, and the log is shorter than its contents.
for (let i = 0; i < 8; i++) {
  const sentOne = await api('/api/chat', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ text: `история ${i}` }),
  });
  if (sentOne[0] !== 200) check(`history message ${i} is accepted`, false, JSON.stringify(sentOne));
}
await sleep(900);
await send('Page.reload');
await sleep(2400);
const chatEnd = await evalJs(`(() => {
  const e = document.querySelector('#clog');
  const last = [...e.querySelectorAll('.you')].at(-1);
  const lr = last && last.getBoundingClientRect();
  const er = e.getBoundingClientRect();
  const reps = [...e.querySelectorAll('.rep')];
  const rr = reps.at(-1) && reps.at(-1).getBoundingClientRect();
  const visible = (r) => !!r && r.bottom <= er.bottom + 1 && r.top >= er.top - 1;
  return {
    overflow: e.scrollHeight > e.clientHeight + 20,
    atEnd: e.scrollHeight - e.scrollTop - e.clientHeight < 4,
    lastVisible: visible(lr),
    replyVisible: visible(rr),
    lastText: last?.innerText.slice(0, 80) || '',
    scrollTop: e.scrollTop, scrollHeight: e.scrollHeight, clientHeight: e.clientHeight,
  };
})()`);
check('a long chat history opens at the newest message', chatEnd.overflow && chatEnd.atEnd && chatEnd.lastVisible && chatEnd.replyVisible, JSON.stringify(chatEnd));
await evalJs('document.querySelector("#ask-in").value = "после перезагрузки"; document.querySelector("#ask").requestSubmit()');
check('sending scrolls the chat to the new message', await waitFor(`(() => {
  const e = document.querySelector('#clog');
  const texts = [...e.querySelectorAll('.you .you-text')];
  return texts.at(-1)?.textContent.includes('после перезагрузки') && e.scrollHeight - e.scrollTop - e.clientHeight < 4;
})()`, 8000), await evalJs(`(() => { const e = document.querySelector('#clog'); return { top: e.scrollTop, h: e.scrollHeight, c: e.clientHeight }; })()`));
await click('[data-act="archive-view"]');
check('the archive view lists tasks', await waitFor('document.querySelectorAll(".arc-row").length > 0', 5000), await evalJs('document.querySelectorAll(".arc-row").length'));
const archivedBefore = await stateApi();
await click('.arc-row [data-act="restore"]');
check('restore returns a task to the board', await waitFor(`fetch('/api/state').then(r=>r.json()).then(s => s.archived.length === ${archivedBefore.archived.length - 1})`, 8000));
await click('[data-act="archive-close"]');

// Theme changes are picked up without a reload.
writeFileSync(join(fixture, 'theme', 'colors.toml'), readFileSync(join(fixture, 'theme', 'colors.toml'), 'utf8').replace('accent = "#dfa360"', 'accent = "#ff0000"'));
check('a theme rewrite changes the world palette', await waitFor(`world.debug().palette.accent === '#ff0000'`, 14000), JSON.stringify((await debug())?.palette));

// Narrow: canvas full screen, panels are overlays, chat docks.
await send('Emulation.setDeviceMetricsOverride', { width: 620, height: 900, screenWidth: 620, screenHeight: 900, deviceScaleFactor: 1, mobile: false });
await sleep(500); await evalJs('window.dispatchEvent(new Event("resize")); true'); await sleep(900);
check('below 900px the world is narrow', await evalJs('document.querySelector("#win").classList.contains("narrow")'), `innerWidth=${await evalJs('innerWidth')}`);
check('the canvas keeps the whole screen', await evalJs('Math.abs(document.querySelector("#scene").getBoundingClientRect().width - innerWidth) < 2'));
check('the chat docks at the bottom on a narrow screen', await evalJs('document.querySelector("#console").parentElement.id === "console-dock"'));
// Focus is a z-index:40 overlay at this width; the limits panel must beat it.
await fly('alpha/6');
check('Focus is open on a narrow screen', await waitFor('!document.querySelector("#focus").hidden', 5000));
await clearToasts();
await click('#hud-limits');
check('the limits panel opens on a narrow screen', await waitFor('!!document.querySelector(".pop.limits")', 3000));
const narrowLimits = await evalJs(LIMITS_FIT);
check('the limits panel is above open Focus on a narrow screen', narrowLimits.ok === true && narrowLimits.focusHidden === false, JSON.stringify(narrowLimits));
check('the first window is reachable on a narrow screen', (await text('.pop.limits')).includes('Session (5-hour)') && (await evalJs('document.querySelector(".pop.limits").scrollTop === 0')));
await click('#hud-limits');
check('the chip closes the narrow limits panel', await waitFor('!document.querySelector(".pop.limits")', 3000));
await shot('world-narrow.png');

// A phone-width viewport is the remote case the task asks to keep working.
await send('Emulation.setDeviceMetricsOverride', { width: 500, height: 900, screenWidth: 500, screenHeight: 900, deviceScaleFactor: 1, mobile: false });
await sleep(400); await evalJs('window.dispatchEvent(new Event("resize")); true'); await sleep(700);
await fly('alpha/6');
await waitFor('!document.querySelector("#focus").hidden', 5000);
await clearToasts();
await click('#hud-limits');
check('the limits panel opens at phone width', await waitFor('!!document.querySelector(".pop.limits")', 3000));
const phoneLimits = await evalJs(LIMITS_FIT);
check('the limits panel is above open Focus at phone width', phoneLimits.ok === true && phoneLimits.focusHidden === false, JSON.stringify(phoneLimits));
await click('#hud-limits');
await waitFor('!document.querySelector(".pop.limits")', 3000);
await send('Emulation.clearDeviceMetricsOverride');
await sleep(400);

await navigate(base + '/?calm', 2200);
check('?calm turns the world simple', await evalJs('world.debug().simple === true && document.querySelector("#win").classList.contains("calm")'));
await shot('world-calm.png');
check('no console errors after the whole run', errors.length === 0, errors.slice(0, 3).join('; '));
ws.close();
chrome.kill();
process.exit(failed ? 1 : 0);
