#!/usr/bin/env node
// Checks of the dashboard in headless Chromium over the DevTools protocol.
// No dependencies (Node 22+ has WebSocket). With the server running:
//   FIXTURE=DIR node scripts/ui-check.mjs http://127.0.0.1:7117 [screenshot-dir]
// `scripts/run-checks.sh` starts the server on the world of scripts/fixture.mjs.
// The fixture's logs are the witness for every board command.

import { spawn } from 'node:child_process';
import { mkdtempSync, writeFileSync, readFileSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

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
async function waitForChrome() {
  for (let i = 0; i < 60; i++) {
    try { const r = await fetch(`http://127.0.0.1:${port}/json/version`); if (r.ok) return; } catch {}
    await sleep(200);
  }
  throw new Error('chromium did not start');
}

let ws; let id = 0; const pending = new Map(); const errors = [];
const send = (method, params = {}) => new Promise((r) => { const i = ++id; pending.set(i, r); ws.send(JSON.stringify({ id: i, method, params })); });
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

await waitForChrome();
const tabs = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
const page = tabs.find((t) => t.type === 'page');
ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((r) => { ws.onopen = r; });
ws.onmessage = (e) => {
  const m = JSON.parse(e.data);
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); }
  else if (m.method === 'Runtime.exceptionThrown') errors.push(m.params.exceptionDetails?.text || 'exception');
  else if (m.method === 'Runtime.consoleAPICalled' && m.params.type === 'error') errors.push(m.params.args?.map((a) => a.value || a.description).join(' '));
  else if (m.method === 'Log.entryAdded' && m.params.entry.level === 'error') errors.push(m.params.entry.text);
};
await send('Runtime.enable');
await send('Log.enable');
await send('Page.enable');
await send('Page.navigate', { url: base + '/' });
await sleep(1800);

check('the page has a title and no console errors', (await evalJs('document.title')) === 'Herdr Planner' && errors.length === 0, errors.slice(0, 2).join('; '));
check('the dashboard fills the viewport', await evalJs('Math.abs(document.querySelector(".win").getBoundingClientRect().width - innerWidth) < 2'));
check('every active task is a card', (await evalJs('document.querySelectorAll(".col .card").length')) === 15, String(await evalJs('document.querySelectorAll(".col .card").length')));
const statuses = await evalJs('[...new Set([...document.querySelectorAll(".col .card")].map(c => c.dataset.s))].sort()');
check('the seven card statuses are drawn', ['failed', 'plan', 'queued', 'ready', 'review', 'waiting', 'working'].every((s) => statuses.includes(s)), statuses.join(','));
check('the header counts waiting, projects and quiet ones', /ЖДУТ ТЕБЯ\s+9/i.test(await text('#top')) && /ПРОЕКТОВ\s+4/i.test(await text('#top')) && /ТИХИХ\s+1/i.test(await text('#top')), (await text('#top')).replace(/\s+/g, ' ').slice(0, 220));

// Threads must not leave the visible column or cross the bottom lanes.
const offscreen = await evalJs(`(() => {
  document.querySelectorAll('.col').forEach((c) => { c.scrollTop = c.scrollHeight; });
  layoutThreads();
  const out = [];
  document.querySelectorAll('.col').forEach((col) => {
    const cr = col.getBoundingClientRect();
    col.querySelectorAll('.group').forEach((g) => {
      const h = g.querySelector('.ghead').getBoundingClientRect();
      if (h.bottom <= cr.top + 1 || h.top >= cr.bottom - 1) out.push(g.dataset.repo);
    });
  });
  return out;
})()`);
check('scrolling leaves projects with their headers out of view', offscreen.length > 0, JSON.stringify(offscreen));
check('threads of scrolled-out projects are hidden', await evalJs(`(${JSON.stringify(offscreen)}).every((repo) => {
  const g = document.querySelector('#g-th g[data-repo="' + repo + '"]');
  return !g || g.style.display === 'none';
})`));
check('no thread is drawn over the bottom lanes', !(await evalJs(`(() => {
  const l = document.querySelector('.lanes').getBoundingClientRect();
  return [...document.querySelectorAll('#g-th path, #g-pk path')].some((p) => {
    const r = p.getBoundingClientRect();
    return r.width + r.height > 0 && r.bottom > l.top && r.top < l.bottom;
  });
})()`)));
await evalJs('document.querySelectorAll(".col").forEach((c) => { c.scrollTop = 0; }); layoutThreads(); true');
await sleep(250);
check('a ready card offers "Одобрить"', (await text('.card[data-id="alpha/4"]')).includes('Одобрить'));
check('a failed card offers "Повторить"', (await text('.card[data-id="alpha/8"]')).includes('Повторить'));
check('a waiting card offers "Ответить"', (await text('.card[data-id="alpha/2"]')).includes('Ответить'));
check('a plan card offers "Одобрить план"', (await text('.card[data-id="alpha/3"]')).includes('Одобрить план'));
check('old done tasks fold into "Сделано ещё"', /Сделано ещё 2/.test(await text('.group[data-repo="alpha"]')));

// Focus and details.
await click('.card[data-id="alpha/6"]');
check('a card opens the focus lane', await waitFor('document.querySelector("#laneF").innerText.includes("Миграция базы")'));
check('focus shows the worktree and the branch', await waitFor('document.querySelector("#laneF").innerText.includes("worktrees") && document.querySelector("#laneF").innerText.includes("work/6")'));
check('focus offers the agent message field', await evalJs('!!document.querySelector("#tell")'));

// A confirmation belongs to one task and one command: arming approve must
// not let a single click on «Всё равно влить» merge anything.
await click('.card[data-id="alpha/4"]');
await click('#laneF [data-act="approve"]');
check('approve arms visibly', (await text('#laneF [data-act="approve"]')).includes('Подтвердить'));
await click('#laneF [data-act="anyway"]');
check('anyway does not reuse the approve arm', !calls().some((c) => c.argv[0] === 'approve' && c.argv[1] === 'alpha/4'), JSON.stringify(lastCall()));
check('anyway has its own visible confirmation', (await text('#laneF [data-act="anyway"]')).includes('Подтвердить «всё равно»'));

// Approve needs two clicks; the server also refuses a bare approve.
const bare = await fetch(base + '/api/action', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ action: 'approve', id: 'alpha/3' }) });
check('the API refuses an approval without confirmation', bare.status === 400, String(bare.status));
const bareAnyway = await fetch(base + '/api/action', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ action: 'anyway', id: 'alpha/5' }) });
check('the API refuses anyway without confirmation', bareAnyway.status === 400, String(bareAnyway.status));
const badId = await fetch(base + '/api/action', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ action: 'retry', id: '--all/1' }) });
check('a malformed task id is a 400, not a gateway error', badId.status === 400, String(badId.status));
await click('.card[data-id="alpha/4"]');
await click('.card[data-id="alpha/4"] .btn.go');
check('approve arms on the first click', await waitFor('document.querySelector(\'.card[data-id="alpha/4"] .btn.go\')?.textContent.includes("Подтвердить")'));
await sleep(700);
await click('.card[data-id="alpha/4"] .btn.go');
check('the second click runs herdr-projects approve', await waitFor('fetch("/api/state").then(r=>r.json()).then(s => s.tasks.find(t => t.id === "alpha/4")?.status === "done")', 10000));
check('the done card appears fresh after approval', await waitFor('!!document.querySelector(\'.card.s-done[data-id="alpha/4"]\')', 5000));
check('the board received approve', calls().some((c) => c.argv[0] === 'approve' && c.argv[1] === 'alpha/4'), JSON.stringify(lastCall()));

// «Всё равно влить» shows its confirmation, and a double-click does not
// confirm it; the same card's «Ответить» never becomes an empty armed button.
await click('.card[data-id="alpha/5"]');
await click('#laneF [data-act="anyway"]');
check('anyway arms on the first click', (await text('#laneF [data-act="anyway"]')).includes('Подтвердить «всё равно»'));
const anywayBefore = calls().filter((c) => c.argv[0] === 'approve' && c.argv[1] === 'alpha/5').length;
await evalJs('document.querySelector("#laneF [data-act=\\"anyway\\"]").click(); document.querySelector("#laneF [data-act=\\"anyway\\"]").click(); true');
await sleep(120);
check('a double-click does not send anyway', calls().filter((c) => c.argv[0] === 'approve' && c.argv[1] === 'alpha/5').length === anywayBefore, JSON.stringify(lastCall()));
check('arming anyway keeps the card action readable', (await text('.card[data-id="alpha/5"] .btn.go')).includes('Ответить'), await text('.card[data-id="alpha/5"] .btn.go'));
await sleep(700);
await click('#laneF [data-act="anyway"]');
check('the confirmed click runs approve --anyway', await waitFor('fetch("/api/state").then(r=>r.json()).then(s => s.tasks.find(t => t.id === "alpha/5")?.status === "done")', 10000));
check('the board received approve --anyway', calls().some((c) => c.argv[0] === 'approve' && c.argv[1] === 'alpha/5' && c.argv[2] === '--anyway'), JSON.stringify(lastCall()));

// Focus actions: stop/retry and a message to the agent.
await click('.card[data-id="alpha/8"]');
await click('#laneF .f-actions [data-act="retry"]');
check('retry reaches the board', await waitFor('fetch("/api/state").then(r=>r.json()).then(s => s.tasks.find(t => t.id === "alpha/8")?.status === "working")', 10000));
await click('.card[data-id="alpha/2"]');
await evalJs('document.querySelector("#tell").value = "старую"; document.querySelector("[data-act=\\"tell-agent\\"]").click()');
const told = (text) => calls().some((c) => c.argv[0] === 'tell' && c.argv[1] === 'alpha/2' && c.argv[2] === '--' && c.argv[3] === text);
for (let i = 0; i < 60 && !told('старую'); i++) await sleep(150);
check('a message to the agent reaches herdr-projects tell', told('старую'), JSON.stringify(lastCall()));
const dashTell = await api('/api/action', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ action: 'tell', id: 'alpha/2', text: '-- hi' }) });
check('a message starting with -- is passed as text, not an option', dashTell[0] === 200 && told('-- hi'), JSON.stringify(lastCall()));

// Yazi is local: the fake terminal receives yazi plus the worktree.
await click('.card[data-id="alpha/6"]');
await click('#laneF [data-act="files"]');
for (let i = 0; i < 25 && !calls('terminal.jsonl').some((c) => c.argv[0] === 'yazi' && c.argv[1].includes('worktrees')); i++) await sleep(150);
check('the folder button opens yazi in the worktree', calls('terminal.jsonl').some((c) => c.argv[0] === 'yazi' && c.argv[1].includes('worktrees')), JSON.stringify(lastCall('terminal.jsonl')));

// A non-loopback host is a remote client: file actions show the path instead
// of starting a terminal on the machine with the board.
const remoteURL = `${new URL(base).protocol}//remote.test:${new URL(base).port}/`;
const terminalsBefore = calls('terminal.jsonl').length;
await send('Page.navigate', { url: remoteURL });
await sleep(1600);
check('a non-loopback page is a remote client', await evalJs('state.local') === false);
await click('.card[data-id="alpha/6"]');
check('a remote page still loads task details', await waitFor('document.querySelector("#laneF").innerText.includes("worktrees")', 5000));
await click('#laneF [data-act="files"]');
check('a remote file button shows the path', await waitFor('document.querySelector(".toast")?.innerText.includes("worktrees")', 5000), await text('.toast'));
check('a remote file button does not start yazi', calls('terminal.jsonl').length === terminalsBefore);
await send('Page.navigate', { url: base + '/' });
await sleep(1600);

// Filters.
await click('[data-filter="failed"]');
check('a status chip dims the rest', await evalJs('document.querySelectorAll(".card.dim").length > 0'));
await click('[data-filter="failed"]');
await click('[data-stage="review"]');
check('a stage filters and shows its pressed state', (await evalJs('document.querySelector(\'[data-stage="review"]\').getAttribute("aria-pressed")')) === 'true');
await click('[data-stage="review"]');

// Chat: the fake Coordinator answers.
await evalJs('document.querySelector("#ask-in").value = "проверка чата"; document.querySelector("#ask").requestSubmit()');
check('the chat sends coordinator tell', await waitFor(`fetch('/api/state').then(r => r.json()).then(s => s.coordinator.last_sent === 'проверка чата')`, 8000));
check('the Coordinator answer is shown in the console', await waitFor('document.querySelector("#clog").innerText.includes("Принял")', 10000));
const helpChat = await api('/api/chat', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ text: '--help' }) });
check('a Coordinator message starting with -- is literal', helpChat[0] === 200 && calls('coordinator.jsonl').some((c) => c.text === '--help'), JSON.stringify(lastCall('coordinator.jsonl')));

// Archive: one project's done tasks, then every done task, view and restore.
const beforeProject = await stateApi();
const projectDone = beforeProject.tasks.filter((t) => t.repo === 'alpha' && t.status === 'done').map((t) => t.id);
await click('.group[data-repo="alpha"] [data-archive-repo]');
check('a project archives only its own done tasks', projectDone.length > 0 && await waitFor(`fetch('/api/state').then(r=>r.json()).then(s => ${JSON.stringify(projectDone)}.every(id => s.archived.some(t => t.id === id)))`, 12000), JSON.stringify(projectDone));
check('other projects keep their done tasks', await waitFor(`fetch('/api/state').then(r=>r.json()).then(s => s.tasks.some(t => t.repo !== 'alpha' && t.status === 'done'))`));
const before = await stateApi();
await click('[data-act="archive-done-all"]');
check('"убрать сделанные" archives every done task', await waitFor(`fetch('/api/state').then(r=>r.json()).then(s => s.archived.filter(t => t.archived).length === ${before.archived.length + before.summary.done})`, 12000));
const archived = await stateApi();
check('the archive grew by the done tasks', archived.archived.length === before.archived.length + before.summary.done, `${archived.archived.length}`);
await click('[data-act="archive-view"]');
check('the archive view lists tasks', await waitFor(`document.querySelectorAll('.arc-row').length === ${archived.archived.length}`));
await click('.arc-row [data-act="restore"]');
check('restore returns a task to the board', await waitFor(`fetch('/api/state').then(r=>r.json()).then(s => s.archived.length === ${archived.archived.length - 1})`, 8000));
await click('[data-act="archive-close"]');

// Theme changes are picked up without a reload.
writeFileSync(join(fixture, 'theme', 'colors.toml'), readFileSync(join(fixture, 'theme', 'colors.toml'), 'utf8').replace('accent = "#dfa360"', 'accent = "#ff0000"'));
check('a theme rewrite changes the accent without a reload', await waitFor(`getComputedStyle(document.documentElement).getPropertyValue('--c-accent').trim() === '#ff0000'`, 12000));

// Layout modes.
await send('Emulation.setDeviceMetricsOverride', { width: 900, height: 900, screenWidth: 900, screenHeight: 900, deviceScaleFactor: 1, mobile: false });
await sleep(400); await evalJs('window.dispatchEvent(new Event("resize")); true'); await sleep(800);
check('below 1120px the window is stacked', await evalJs('document.querySelector(".win").classList.contains("stacked")'), `innerWidth=${await evalJs('innerWidth')}, win=${await evalJs('document.querySelector(".win").getBoundingClientRect().width')}, mq=${await evalJs('matchMedia("(max-width:1120px)").matches')}`);
await send('Emulation.setDeviceMetricsOverride', { width: 620, height: 900, screenWidth: 620, screenHeight: 900, deviceScaleFactor: 1, mobile: false });
await sleep(400); await evalJs('window.dispatchEvent(new Event("resize")); true'); await sleep(800);
check('below 860px the window is narrow', await evalJs('document.querySelector(".win").classList.contains("narrow")'), `innerWidth=${await evalJs('innerWidth')}, win=${await evalJs('document.querySelector(".win").getBoundingClientRect().width')}, mq=${await evalJs('matchMedia("(max-width:1120px)").matches')}`);
await send('Emulation.clearDeviceMetricsOverride');
await sleep(400);

await send('Page.navigate', { url: base + '/?calm' });
await sleep(1600);
check('?calm turns motion off', await evalJs('document.querySelector(".win").classList.contains("calm") && getComputedStyle(document.querySelector(".card") || document.body).animationName === "none"'));
check('no console errors after the whole run', errors.length === 0, errors.slice(0, 3).join('; '));
await shot('dashboard.png');
ws.close();
chrome.kill();
process.exit(failed ? 1 : 0);
