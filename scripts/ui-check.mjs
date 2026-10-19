#!/usr/bin/env node
// Checks of the page in headless Chromium over the DevTools protocol.
// No dependencies (Node 22+ has WebSocket). Usage, with the server running:
//   node scripts/ui-check.mjs [http://127.0.0.1:7117] [screenshot-dir]
// Exits 1 when a check fails. Needs `chromium` (or $CHROMIUM) on the PATH.
//
// Against a live server only the list and a read-only look at the first
// project's card and the settings screen (nothing is changed) are checked. With
// FIXTURE=DIR (a server started on the world of scripts/fixture.mjs;
// `scripts/run-checks.sh` does all of it) the whole page is checked: the
// stages of the board, the counters, the task panel and the board's commands
// (approve, review, retry, delegate, stop, tell), the bar popup's data, the
// workspace follow and the settings — with the fake binaries' logs as the witness.

import { spawn } from 'node:child_process';
import net from 'node:net';
import { mkdtempSync, writeFileSync, rmSync, readFileSync, existsSync, appendFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const base = process.argv[2] || 'http://127.0.0.1:7117';
const shots = process.argv[3] || null;
const fixture = process.env.FIXTURE || null;
const port = 9333 + Math.floor(Math.random() * 500);
const profile = mkdtempSync(join(tmpdir(), 'herdr-planner-ui-'));
const chrome = spawn(process.env.CHROMIUM || 'chromium', [
  '--headless=new', '--no-sandbox', '--disable-gpu', '--hide-scrollbars', `--user-data-dir=${profile}`,
  `--remote-debugging-port=${port}`, '--window-size=1180,820', 'about:blank',
], { stdio: 'ignore' });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let failed = 0;
function check(name, ok, extra = '') {
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${name}${extra ? ' — ' + extra : ''}`);
  if (!ok) failed++;
}

async function waitForChrome() {
  for (let i = 0; i < 50; i++) {
    try {
      const r = await fetch(`http://127.0.0.1:${port}/json/version`);
      if (r.ok) return;
    } catch {}
    await sleep(200);
  }
  throw new Error('chromium did not start');
}


let ws;
let id = 0;
const pending = new Map();
const send = (method, params = {}) => new Promise((r) => { const i = ++id; pending.set(i, r); ws.send(JSON.stringify({ id: i, method, params })); });
const evalJs = async (expr) => (await send('Runtime.evaluate', { expression: expr, returnByValue: true, awaitPromise: true })).result?.result?.value;
const key = async (k, text, physical) => {
  const code = { ArrowDown: 40, ArrowUp: 38, Enter: 13, Escape: 27, ArrowLeft: 37, ArrowRight: 39, Tab: 9 }[k];
  // Shortcuts are handled on keydown; a `char` event would type the letter even
  // where the page cancels the default, so text is inserted only when asked.
  const common = code ? { key: k, code: k, windowsVirtualKeyCode: code, nativeVirtualKeyCode: code } : (physical ? { key: k, code: physical } : { key: k });
  await send('Input.dispatchKeyEvent', { type: code ? 'rawKeyDown' : 'keyDown', ...common });
  if (text) await send('Input.dispatchKeyEvent', { type: 'char', text, key: k });
  await send('Input.dispatchKeyEvent', { type: 'keyUp', ...common });
  await sleep(120);
};
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
const text = (sel) => evalJs(`document.querySelector(${JSON.stringify(sel)})?.innerText || ''`);
const ids = () => evalJs('[...document.querySelectorAll(".list .row.task")].map((r) => r.getAttribute("data-id"))');
const logLines = (name) => {
  const f = join(fixture, 'log', name);
  return existsSync(f) ? readFileSync(f, 'utf8').split('\n').filter(Boolean).map((l) => JSON.parse(l)) : [];
};
const api = (method, path, body) => evalJs(`fetch(${JSON.stringify(path)}, { method: ${JSON.stringify(method)}, headers: { 'content-type': 'application/json' }, body: ${body === undefined ? 'undefined' : JSON.stringify(JSON.stringify(body))} }).then(async (r) => [r.status, await r.json().catch(() => null)])`);
const click = (sel) => evalJs(`(() => { const e = document.querySelector(${JSON.stringify(sel)}); if (e) e.click(); return !!e; })()`);
const note = () => text('.note');


const typed = async (s) => { await send('Input.insertText', { text: s }); await sleep(150); };
const snapshot = () => evalJs('fetch("/api/snapshot").then(r => r.json())');
const refresh = async () => { await evalJs('fetch("/api/refresh", { method: "POST" })'); };
const rowText = (id) => text(`.row[data-id="${id}"]`).then((t) => t.replace(/\s+/g, ' '));
const slugs = (l) => l.map((p) => p.slug);
const board = () => JSON.parse(readFileSync(join(fixture, 'state/board.json'), 'utf8'));
const calls = () => logLines('calls.jsonl');
const lastCall = () => calls().at(-1);

// The herdr server's socket: one JSON line in, one out; the page's «показать все» goes here.
let sockServer = null;
function startSocket() {
  const path = join(fixture, 'herdr.sock');
  sockServer = net.createServer((c) => {
    let buf = '';
    c.on('data', (d) => {
      buf += d;
      if (!buf.includes('\n')) return;
      const req = JSON.parse(buf.split('\n')[0]);
      appendFileSync(join(fixture, 'log', 'socket.jsonl'), JSON.stringify(req) + '\n');
      c.end(JSON.stringify({ id: req.id, result: { type: 'agent_view', active: false } }) + '\n');
    });
  });
  sockServer.listen(path);
}

// The card of a live project, read-only: what is there is there.
async function cardSmoke() {
  check('card has tabs and a filter', await evalJs('!!document.querySelector(".tabs") && !!document.querySelector("#cfilter")'));
  const overflow = await evalJs('document.scrollingElement.scrollWidth - window.innerWidth');
  check('card does not scroll horizontally', overflow <= 0, `overflow ${overflow}px`);
  await key('2');
  check('2 opens «Сделано»', /Сделано/.test(await text('.tab.active')));
  await key('1');
  check('1 opens «В деле»', /В деле/.test(await text('.tab.active')));
}

// ----- the list against the board of the fixture
async function boardChecks() {
  const ov = await evalJs('Promise.all([fetch("/api/overview").then(r => r.json()), fetch("/api/summary").then(r => r.json()), fetch("/api/snapshot").then(r => r.json())]).then(([o, m, s]) => ({o, m, s}))');
  const proj = (slug) => ov.s.projects.find((p) => p.slug === slug);
  check('the projects are the repos of the board (and one that only has tasks)', ov.s.projects.length === 12 && !!proj('orphan') && proj('alpha').key === 'ALP' && proj('orphan').key === 'ORP' && proj('orphan').path === '', JSON.stringify(slugs(ov.s.projects)));
  const a = proj('alpha');
  check('alpha: its tasks by stage', JSON.stringify(Object.entries(a.counts).sort()) === JSON.stringify(Object.entries({ needs: 5, ready: 1, failed: 1, review: 1, working: 1, queued: 2, todo: 1, done: 2 }).sort()), JSON.stringify(a.counts));
  check('alpha: the tasks come needs-you first, the newest first inside a stage, done last',
    JSON.stringify(a.tasks.map((t) => t.id.split('/')[1])) === JSON.stringify(['2', '3', '5', '14', '12', '4', '8', '7', '6', '9', '10', '11', '1', '13']), JSON.stringify(a.tasks.map((t) => t.id.split('/')[1])));
  const stage = Object.fromEntries(a.tasks.map((t) => [t.id.split('/')[1], t.label]));
  check('…each stage in words: plan ready, review failed, ready to approve, failed, queued, queued after, pick one',
    stage[3] === 'план готов' && stage[5] === 'ревью не прошло' && stage[4] === 'готова к одобрению' && stage[8] === 'упала' && stage[9] === 'в очереди, №1' && stage[10] === 'ждёт другие задачи' && stage[12] === 'выбери вариант' && stage[2] === 'ждёт ответа', JSON.stringify(stage));
  check('tags come from the board (ALP-2); a repo with no key shows #n', a.tasks[0].tag === 'ALP-2' && !!ov.s.projects.length, a.tasks[0].tag);
  check('iota: a task that stopped without a summary has failed; zeta: finished work is ready; beta has nothing', proj('iota').counts.failed === 1 && proj('zeta').counts.ready === 1 && proj('beta').tasks.length === 0);
  check('overview: «ждут тебя» by arrival, the newest first (kappa 2 min, zeta 5, alpha 6, iota 10, sigma 100)', JSON.stringify(slugs(ov.o.waiting)) === JSON.stringify(['kappa', 'zeta', 'alpha', 'iota', 'sigma']), JSON.stringify(slugs(ov.o.waiting)));
  check('overview: «на ревью» (rho 8 min, eps 40 min) and «работают» (theta); alpha is listed once, with the waits', JSON.stringify(slugs(ov.o.review)) === JSON.stringify(['rho', 'eps']) && JSON.stringify(slugs(ov.o.working)) === JSON.stringify(['theta']), JSON.stringify([slugs(ov.o.review), slugs(ov.o.working)]));
  check('overview: quiet projects are counted, not listed (beta, gamma, eta, orphan)', ov.o.quiet === 4 && !('groups' in ov.o), `quiet ${ov.o.quiet}`);
  const times = ov.o.waiting.map((p) => p.attention_since);
  check('overview: attention_since is the time the task came to its stage, falling down the list', times.every((t) => t > 1.7e12) && times[0] > times[1] && times[1] > times[2] && times[2] > times[3] && times[3] > times[4], JSON.stringify(times));
  check('overview: a project under review shows when its review began, the newest first (rho 8 min, eps 40 min)', ov.o.review.every((p) => p.since > 1.7e12) && ov.o.review[0].since > ov.o.review[1].since && ov.o.review[0].attention_since === 0 && ov.o.waiting.every((p) => p.since === p.attention_since) && ov.o.working[0].since === 0, JSON.stringify(ov.o.review.map((p) => [p.slug, p.since])));
  const sg = Object.fromEntries(proj('sigma').tasks.map((t) => [t.id.split('/')[1], t]));
  check('sigma: a stop by hand is «остановлена» and can be delegated again; a stop at a limit waits for you and can be continued (retry)', sg[1].bucket === 'todo' && sg[1].label === 'остановлена' && sg[1].actions.join() === 'delegate' && sg[2].bucket === 'needs' && sg[2].label === 'остановлена на лимите' && sg[2].actions.join() === 'retry,tell', JSON.stringify([sg[1], sg[2]].map((t) => [t.label, t.actions])));
  check('sigma: work whose pull request checks fail is not «готова к одобрению»: it is «нельзя влить» and can be approved anyway; a reviewer that asks waits for you', sg[3].bucket === 'needs' && sg[3].label === 'нельзя влить' && sg[3].actions.join() === 'anyway,review,tell' && sg[4].bucket === 'needs' && sg[4].label === 'ревьюеру нужен ты', JSON.stringify([sg[3], sg[4]].map((t) => [t.label, t.actions])));
  const ageMin = (Date.now() - times[0]) / 60000;
  check('overview: kappa\'s question is 2 minutes old', ageMin > 1.2 && ageMin < 6, `${ageMin.toFixed(1)} min`);
  const w = ov.o.waiting;
  check('overview: each waiting project says what waits (tag and stage)', w[0].wait_hint === 'KAP-1: ждёт ответа' && w[1].wait_hint === 'ZET-1: готова к одобрению' && w[2].wait_hint === 'ALP-2: ждёт ответа; ALP-4: готова к одобрению; ALP-3: план готов; ALP-5: ревью не прошло; ALP-14: ревью не прошло; ALP-8: упала; ALP-12: выбери вариант' && w[3].wait_hint === 'IOT-1: остановилась без итога', JSON.stringify(w.map((p) => p.wait_hint)));
  check('overview: alpha counts for the popup (waiting = needs + ready + failed), and its lines say what goes on',
    w[2].waiting === 7 && w[2].ready === 1 && w[2].failed === 1 && w[2].review === 1 && w[2].working === 3 && w[2].done === 2 && w[2].total === 14
    && w[2].tasks.length === 5 && w[2].tasks[0].tag === 'ALP-2' && w[2].tasks[0].bucket === 'needs' && !w[2].tasks.some((t) => t.bucket === 'todo' || t.bucket === 'done'), JSON.stringify(w[2]).slice(0, 400));
  const wk = ov.o.working[0].tasks[0];
  check('overview: a working task carries its progress; a review does not', wk.progress === 30 && wk.bucket === 'working' && ov.o.review[0].tasks[0].progress === null, JSON.stringify([wk, ov.o.review[0].tasks[0]]));
  const m = ov.m;
  check('summary: waiting 13 (9 need you, 2 ready, 2 failed), review 3, working 2, queued 2, 12 projects',
    m.waiting === 13 && m.needs === 9 && m.ready === 2 && m.failed === 2 && m.review === 3 && m.working === 2 && m.queued === 2 && m.projects === 12 && m.attention_since === times[0], JSON.stringify(m));
  check('summary: the hint names the projects that wait, the newest first', /^kappa: KAP-1: ждёт ответа\nzeta: ZET-1/.test(m.waiting_hint), m.waiting_hint);
  check('/api/summary is small and has the fields the bar icon reads', Object.keys(m).length <= 16 && ['waiting', 'review', 'updated', 'error'].every((k) => k in m), JSON.stringify(Object.keys(m)));
  const lists = logLines('lists.jsonl');
  check('the board is read with `list --json` (and the plain `list` for the tags), in the folder that was named', lists.some((l) => l.argv.includes('--json')) && lists.some((l) => !l.argv.includes('--json')) && lists.every((l) => l.data === join(fixture, 'board')), JSON.stringify(lists.at(-1)));
  const totals = (await text('.totals')).replace(/\s+/g, ' ');
  check('the header counts the board\'s tasks: waiting, ready, failed, review, working, queued', totals === '9 ждут тебя · 2 к одобрению · 2 упали · 3 на ревью · 2 в работе · 2 в очереди', totals);
  const rowKappa = await evalJs('(document.querySelector(\'.row[data-row="kappa"] .warning.strong\')?.textContent || "") + "|" + (document.querySelector(\'.row[data-row="kappa"] .ago\')?.textContent || "") + "|" + (document.querySelector(\'.row[data-row="kappa"] .detail\')?.textContent || "")');
  check('the page row: «1 ждут», how long ago, and what waits as the second line', /^1 ждут\s*\|(\d+ мин|сейчас)\s*\|KAP-1: ждёт ответа$/.test(rowKappa || ''), rowKappa);
  const rowAlpha = (await rowText0('alpha'));
  check('alpha\'s row: counts by stage and done/all', /5 ждут/.test(rowAlpha) && /1 к одобр\./.test(rowAlpha) && /1 упала/.test(rowAlpha) && /1 ревью/.test(rowAlpha) && /1 в работе/.test(rowAlpha) && /2 в очереди/.test(rowAlpha) && /2\/14/.test(rowAlpha), rowAlpha);
  const first = await evalJs('(() => { document.querySelector("[data-act=sort]").click(); return [...document.querySelectorAll(".row[data-row]")].slice(0, 5).map((r) => r.getAttribute("data-row")); })()');
  check('sorted by attention the page puts the newest event on top', JSON.stringify(first.slice(0, 4)) === JSON.stringify(['kappa', 'zeta', 'alpha', 'iota']), JSON.stringify(first));
  await evalJs('document.querySelector("[data-act=sort]").click()');
  const quiet = await evalJs('[...document.querySelectorAll(".row.quiet[data-row]")].map((r) => r.getAttribute("data-row")).sort().join()');
  check('quiet projects are marked quiet on the page', quiet === 'beta,eta,gamma,orphan', quiet);
}

const rowText0 = (slug) => text(`.row[data-row="${slug}"]`).then((t) => t.replace(/\s+/g, ' '));

// ----- the list's own keys against the fixture: chips, filter, unfocus
async function listChecks() {
  await key('2');
  check('2 shows the projects that wait', /Ждут/.test(await text('.badge.active')) && JSON.stringify(await evalJs('[...document.querySelectorAll(".row[data-row]")].map((r) => r.getAttribute("data-row")).sort()')) === JSON.stringify(['alpha', 'iota', 'kappa', 'sigma', 'zeta']));
  await key('3');
  check('3 shows those in progress (alpha, eps, rho, theta; orphan\'s task is not started)', JSON.stringify(await evalJs('[...document.querySelectorAll(".row[data-row]")].map((r) => r.getAttribute("data-row")).sort()')) === JSON.stringify(['alpha', 'eps', 'rho', 'theta']));
  await key('4');
  check('4 shows the quiet ones', JSON.stringify(await evalJs('[...document.querySelectorAll(".row[data-row]")].map((r) => r.getAttribute("data-row")).sort()')) === JSON.stringify(['beta', 'eta', 'gamma', 'orphan']));
  const chips = await evalJs('[...document.querySelectorAll("[data-chip]")].map((c) => c.textContent.replace(/[●\\s]+/g, " ").trim())');
  check('the chips carry their counts', JSON.stringify(chips) === JSON.stringify(['Все 12', 'Ждут 5', 'В работе 4', 'Тихие 4']), JSON.stringify(chips));
  await key('1');
  await key('/');
  await typed('KAP');
  check('the filter finds a project by its key', JSON.stringify(await evalJs('[...document.querySelectorAll(".row[data-row]")].map((r) => r.getAttribute("data-row"))')) === JSON.stringify(['kappa']));
  await key('Escape');
  await evalJs('(() => { const f = document.querySelector("#filter"); f.value = ""; f.dispatchEvent(new Event("input", { bubbles: true })); })()');
  await key('Escape');
  // «показать все проекты» clears the agent view of herdr: a request to its socket
  const before = logLines('socket.jsonl').length;
  await key('u');
  const cleared = await waitFor('/снова показывает все проекты/.test(document.querySelector(".note")?.innerText || "")');
  const sock = logLines('socket.jsonl');
  check('U clears the sidebar view through the herdr socket (agent.view.clear)', cleared && sock.length === before + 1 && sock.at(-1).method === 'agent.view.clear', JSON.stringify(sock.at(-1)));
  await click('[data-act=unfocus]');
  for (let i = 0; i < 25 && logLines('socket.jsonl').length < before + 2; i++) await sleep(100);
  check('the «все проекты» switch does the same by click', logLines('socket.jsonl').length === before + 2);
  check('…and no command of the board is run for it', calls().length === 0, JSON.stringify(calls()));
  await key('?');
  check('? lists the keys: U shows all, there is no focus or new-project key any more', /U\s*показать все проекты/i.test(await text('.dock')) && !/фокус|новый проект/i.test(await text('.dock')), await text('.dock'));
  await key('?');
  const [s1, e1] = await api('POST', '/api/action', { kind: 'focus', slug: 'alpha' });
  const [s2] = await api('POST', '/api/action', { kind: 'show', slug: 'нет такого' });
  check('there is no «focus» or «open» action any more; «show» wants a project of the board', s1 === 400 && s2 === 404 && calls().length === 0, JSON.stringify([s1, e1, s2]));
}

// ----- the card of alpha: the stages, the panel of a task and the board's commands
async function cardChecks() {
  const f = check;
  const title = await text('.header .title');
  f('card header names the project and its key', /alpha\s+ALP/.test(title), title);
  f('the folder is shown', /\/repos\/alpha/.test(await text('.kv')), await text('.kv'));
  const progress = (await text('.progress')).replace(/\s+/g, ' ');
  f('progress counts the board\'s tasks and the stages', /^2 из 14 сделано · 5 ждут тебя · 1 к одобрению · 1 упала · 1 на ревью · 1 в работе · 2 в очереди$/.test(progress), progress);
  const heads = await evalJs('[...document.querySelectorAll(".sect")].map((s) => s.innerText.replace(/\\s+/g, " ").trim())');
  f('the stages are headed in the order they need the user', JSON.stringify(heads.map((s) => s.replace(/^\S+\s/, ''))) === JSON.stringify(['Ждут тебя 5', 'Готовы к одобрению 1', 'Упали 1', 'На ревью 1', 'В работе 1', 'В очереди 2', 'Не начаты 1']), JSON.stringify(heads));
  f('the open tasks are listed in that order (the done ones are on the other tab)', JSON.stringify(await ids()) === JSON.stringify(['alpha/2', 'alpha/3', 'alpha/5', 'alpha/14', 'alpha/12', 'alpha/4', 'alpha/8', 'alpha/7', 'alpha/6', 'alpha/9', 'alpha/10', 'alpha/11']), JSON.stringify(await ids()));
  f('a task row: tag, title, stage in words, progress, minutes left', /ALP-6\s+Миграция базы/.test(await rowText('alpha/6')) && /в работе\s+40%\s+· ещё 12 мин/.test(await rowText('alpha/6')) && /Writing migration/.test(await rowText('alpha/6')), await rowText('alpha/6'));
  f('…review round and pull request for finished work', /готова к одобрению.*R#1.*PR#12/.test(await rowText('alpha/4')), await rowText('alpha/4'));
  f('…a failed review is red and says so; the plan and the pick wait too', /ревью не прошло.*R#2/.test(await rowText('alpha/5')) && await evalJs('!!document.querySelector(".row[data-id=\\"alpha/5\\"] .value .error")') && /план готов/.test(await rowText('alpha/3')) && /выбери вариант/.test(await rowText('alpha/12')), await rowText('alpha/5'));
  f('…a queued task says its place', /в очереди, №1/.test(await rowText('alpha/9')));
  const overflow = await evalJs('document.scrollingElement.scrollWidth - window.innerWidth');
  f('card does not scroll horizontally', overflow <= 0, `overflow ${overflow}px`);
  await shot('card-todo.png');

  // the panel of a ready task
  await click('.row[data-id="alpha/4"]');
  const read = await waitFor('/Ветка\\s+work\\/4/.test(document.querySelector(".panel")?.innerText || "")');
  const panel = (await text('.panel')).replace(/\s+/g, ' ');
  f('a click opens the task: stage, branch, worktree, pull request, agent, time', read && /Стадия\s+готова к одобрению/.test(panel) && /work\/4/.test(panel) && /\/fixture\/worktrees\/work-4/.test(panel) && /pull\/12/.test(panel) && /Агент\s+claude/.test(panel) && /Время агента\s+4m/.test(panel), panel.slice(0, 300));
  f('…the review: passed, with its tally and findings', /Ревью: прошло · 1 minor/.test(panel) && /minor Название кнопки можно короче/.test(panel), panel);
  f('…the commands of the stage: approve, send the review, reply — and nothing else', JSON.stringify(await evalJs('[...document.querySelectorAll(".panel [data-c=cmd]")].map((b) => b.getAttribute("data-id"))')) === JSON.stringify(['approve', 'anyway', 'review', 'tell']));
  await shot('card-panel.png');
  const n0 = calls().length;
  await key('a');
  f('approving asks first', /Одобрить и влить работу\?/.test(await text('.panel .actions')) && calls().length === n0, await text('.panel .actions'));
  await key('n');
  f('n leaves it: nothing was sent', !(await evalJs('!!document.querySelector(".panel [data-c=approve-yes]")')) && calls().length === n0);
  await key('a');
  await key('y');
  const merged = await waitFor('/одобрить — ALP-4: approved/.test(document.body.innerText)', 6000);
  const c = lastCall();
  f('y approves: `approve alpha/4` through the board, its answer is shown', merged && calls().length === n0 + 1 && JSON.stringify(c.argv) === JSON.stringify(['approve', 'alpha/4']), JSON.stringify(c));
  const moved = await waitFor('!document.querySelector(".row[data-id=\\"alpha/4\\"]")', 6000);
  f('…and the task leaves the list when the board has merged it', moved && board().rows.find((r) => r.id === 'alpha/4').state === 'done');
  f('…the panel closed with it', !(await evalJs('!!document.querySelector(".panel")')));

  // work that failed its review: send it back, or merge it anyway
  await click('.row[data-id="alpha/5"]');
  await waitFor('/Кэш не сбрасывается/.test(document.querySelector(".panel")?.innerText || "")');
  const p5 = (await text('.panel')).replace(/\s+/g, ' ');
  f('a failed review shows the verdict, the tally and the findings', /Ревью: не прошло · 1 critical, 1 major/.test(p5) && /critical Кэш не сбрасывается/.test(p5) && /cache\.rs:40/.test(p5) && /кругов: 2/.test(p5), p5);
  f('…the commands: send the review back, approve anyway (no plain approve), reply', JSON.stringify(await evalJs('[...document.querySelectorAll(".panel [data-c=cmd]")].map((b) => b.getAttribute("data-id") + ":" + b.textContent.trim())')) === JSON.stringify(['review:Отправить ревью агенту', 'anyway:Одобрить всё равно', 'tell:Ответить']));
  await key('a');
  f('a plain approve key does nothing there', !(await evalJs('!!document.querySelector(".panel [data-c=approve-yes]")')));
  await key('w');
  f('approving anyway says why it asks', /Ревью не прошло\. Влить всё равно\?/.test(await text('.panel .actions')), await text('.panel .actions'));
  await key('y');
  await waitFor('/одобрить всё равно — ALP-5: approved anyway/.test(document.body.innerText)', 6000);
  f('…and passes --anyway on', JSON.stringify(lastCall().argv) === JSON.stringify(['approve', 'alpha/5', '--anyway']), JSON.stringify(lastCall()));
  await click('.row[data-id="alpha/14"]');
  await waitFor('/ALP-14/.test(document.querySelector(".panel")?.innerText || "")');
  await key('v');
  await waitFor('/отправить ревью агенту — ALP-14/.test(document.body.innerText)', 6000);
  f('v sends the review back to the agent: `review alpha/14` (no confirmation)', JSON.stringify(lastCall().argv) === JSON.stringify(['review', 'alpha/14']), JSON.stringify(lastCall()));
  const back = await waitFor('/в работе/.test(document.querySelector(".row[data-id=\\"alpha/14\\"]")?.innerText || "")', 6000);
  f('…the task is with the agent again', back, await rowText('alpha/14'));

  // a plan
  await click('.row[data-id="alpha/3"]');
  await waitFor('/Снять копию базы/.test(document.querySelector(".panel")?.innerText || "")');
  f('a plan is shown, and approving it is "accept the plan"', /Прогнать миграцию/.test(await text('.panel')) && /Принять план/.test(await text('.panel .actions')));
  await key('a');
  f('…with its own question', /Принять план\? Агент начнёт работу\./.test(await text('.panel .actions')));
  await click('[data-c=approve-yes]');
  await waitFor('/принять план|одобрить — ALP-3/.test(document.body.innerText)', 6000);
  f('…the board gets the same `approve`', JSON.stringify(lastCall().argv) === JSON.stringify(['approve', 'alpha/3']));

  // an error: retry
  await click('.row[data-id="alpha/8"]');
  await waitFor('/Сборка упала/.test(document.querySelector(".panel")?.innerText || "")');
  const p8 = await text('.panel');
  f('a failed task shows the board\'s failure; markup in it stays text', /Сбой: Сборка упала/.test(p8) && /npm test: 3 failed/.test(p8) && /<script>alert\(1\)<\/script>/.test(p8) && !(await evalJs('!!document.querySelector(".panel script")')), p8.slice(0, 200));
  f('…the commands: retry, reply, stop', JSON.stringify(await evalJs('[...document.querySelectorAll(".panel [data-c=cmd]")].map((b) => b.getAttribute("data-id"))')) === JSON.stringify(['retry', 'tell', 'stop']));
  await key('e');
  await waitFor('/повторить — ALP-8/.test(document.body.innerText)', 6000);
  f('e retries: `retry alpha/8`', JSON.stringify(lastCall().argv) === JSON.stringify(['retry', 'alpha/8']), JSON.stringify(lastCall()));

  // a task nobody started: delegate; a working one: stop
  await click('.row[data-id="alpha/11"]');
  await waitFor('/ALP-11/.test(document.querySelector(".panel")?.innerText || "")');
  f('a task not started offers «Запустить» only', JSON.stringify(await evalJs('[...document.querySelectorAll(".panel [data-c=cmd]")].map((b) => b.getAttribute("data-id"))')) === JSON.stringify(['delegate']));
  await key('d');
  await waitFor('/запустить — ALP-11/.test(document.body.innerText)', 6000);
  f('d delegates: `delegate alpha/11`', JSON.stringify(lastCall().argv) === JSON.stringify(['delegate', 'alpha/11']));
  await click('.row[data-id="alpha/6"]');
  await waitFor('/ALP-6/.test(document.querySelector(".panel")?.innerText || "")');
  await key('x');
  await waitFor('/остановить — ALP-6/.test(document.body.innerText)', 6000);
  f('x stops the agent: `stop alpha/6`', JSON.stringify(lastCall().argv) === JSON.stringify(['stop', 'alpha/6']));
  await waitFor('/не начата/.test(document.querySelector(".row[data-id=\\"alpha/6\\"]")?.innerText || "")', 6000);

  // a question: reply to the agent
  await click('.row[data-id="alpha/2"]');
  await waitFor('/ALP-2/.test(document.querySelector(".panel")?.innerText || "")');
  f('a question shows what the agent asks', /Какую схему брать/.test(await text('.panel')));
  await key('m');
  f('m opens the reply field', await evalJs('document.activeElement?.id === "ctell"'));
  await typed('--now делай по новой схеме');
  await key('Enter');
  await waitFor('/ответить — ALP-2: sent/.test(document.body.innerText)', 6000);
  f('Enter sends it: `tell alpha/2 -- <text>` (words that look like options stay words)', JSON.stringify(lastCall().argv) === JSON.stringify(['tell', 'alpha/2', '--', '--now делай по новой схеме']), JSON.stringify(lastCall()));
  f('…the field is empty and closed again', !(await evalJs('!!document.querySelector("#ctell")')));

  // a choice between attempts is made on the board
  await click('.row[data-id="alpha/12"]');
  await waitFor('/ALP-12/.test(document.querySelector(".panel")?.innerText || "")');
  f('«pick one» offers no command here and says where to pick', (await evalJs('document.querySelectorAll(".panel [data-c=cmd]").length')) === 0 && /herdr-projects pick alpha\/12/.test(await text('.panel')), await text('.panel'));
  await key('Escape');
  f('esc closes the panel', !(await evalJs('!!document.querySelector(".panel")')));

  // the board refuses: its words are shown, nothing moves
  writeFileSync(join(fixture, 'data', 'refuse'), '');
  await click('.row[data-id="alpha/9"]');
  await waitFor('/ALP-9/.test(document.querySelector(".panel")?.innerText || "")');
  const before = calls().length;
  await key('x');
  await waitFor('/не вышло — .*merge has a conflict/.test(document.body.innerText)', 6000);
  rmSync(join(fixture, 'data', 'refuse'));
  f('a refusal of the board is shown with its words and changes nothing', calls().length === before + 1 && board().rows.find((r) => r.id === 'alpha/9').stage === 'queued', (await note()));
  await key('Escape');

  // the other tab
  await key('2');
  f('2 opens «Сделано»: the finished ones, the newest first', /Сделано\s*4/.test(await text('.tab.active')) && JSON.stringify(await ids()) === JSON.stringify(['alpha/4', 'alpha/5', 'alpha/1', 'alpha/13']), JSON.stringify(await ids()));
  f('a done task shows its pull request quietly and has no commands', /PR#7/.test(await rowText('alpha/1')));
  await click('.row[data-id="alpha/1"]');
  await waitFor('/ALP-1/.test(document.querySelector(".panel")?.innerText || "")');
  f('…the panel says there are no commands', /команд нет/.test(await text('.panel')));
  await key('Escape');
  await key('1');
  await key('/');
  await typed('кэш');
  f('the card\'s filter finds a task by words in its title', JSON.stringify(await ids()) === JSON.stringify(['alpha/14']), JSON.stringify(await ids()));
  await key('Escape');
  await evalJs('(() => { const f = document.querySelector("#cfilter"); f.value = ""; f.dispatchEvent(new Event("input", { bubbles: true })); })()');
  await shot('card-after.png');

  // wrong inputs
  const post = (body) => api('POST', '/api/task/action', body);
  const [a1] = await post({ id: 'alpha/13', kind: 'stop' });
  const [a2] = await post({ id: 'alpha/13', kind: 'rm' });
  const [a3] = await post({ id: '--now', kind: 'stop' });
  const [a4] = await post({ id: 'нет/1', kind: 'stop' });
  const [a5] = await post({ id: 'alpha/2', kind: 'tell', text: '   ' });
  const [a6] = await post({ id: 'alpha/12', kind: 'approve' });
  const [a7] = await post({ id: 'alpha/2', kind: 'anyway' });
  const [d1] = await api('GET', '/api/task?id=alpha/13');
  const [d2] = await api('GET', '/api/task?id=--now');
  const [d3] = await api('GET', '/api/task?id=zzz/1');
  f('task commands: a finished task 409, an unknown command 400, an id like an option 400, a task the board does not list 404, empty words 400, approving a pick 409, --anyway where nothing failed 409',
    [a1, a2, a3, a4, a5, a6, a7].join() === '409,400,400,404,400,409,409', [a1, a2, a3, a4, a5, a6, a7].join());
  // Work the board will not merge by a plain yes, and a task a limit stopped.
  const [c1] = await post({ id: 'sigma/3', kind: 'approve' });
  const [c2] = await post({ id: 'sigma/2', kind: 'delegate' });
  const nc = calls().length;
  const [c3, cc3] = await post({ id: 'sigma/3', kind: 'anyway' });
  f('checks that fail: a plain approve 409, «anyway» goes to the board as `approve --anyway`; a limit stop cannot be delegated 409', c1 === 409 && c2 === 409 && c3 === 200 && calls().length === nc + 1 && JSON.stringify(lastCall().argv) === JSON.stringify(['approve', 'sigma/3', '--anyway']) && /approved anyway/.test(cc3.message), JSON.stringify([c1, c2, c3, lastCall()]));
  const [c4] = await post({ id: 'sigma/2', kind: 'retry' });
  f('a task a limit stopped is continued with `retry`', c4 === 200 && JSON.stringify(lastCall().argv) === JSON.stringify(['retry', 'sigma/2']), JSON.stringify(lastCall()));
  f('task details: a listed task 200, an id like an option 400, an unknown one 404', [d1, d2, d3].join() === '200,400,404', [d1, d2, d3].join());
  const cmds = calls().map((c) => c.argv[0]);
  f('none of the refused requests reached the board', !calls().slice(-1).some((c) => c.argv.includes('--now')) && cmds.every((c) => ['approve', 'review', 'retry', 'delegate', 'stop', 'tell'].includes(c)), JSON.stringify(cmds));
  f('every command of the board ran in the folder that was named', calls().every((c) => c.data === join(fixture, 'board')));
}

// ----- the page follows the active herdr workspace and the bar popup
async function followChecks() {
  const setWs = (v) => writeFileSync(join(fixture, 'state/workspace.json'), JSON.stringify(v));
  setWs({ id: 'wK1', label: 'какое-то имя' });
  const byId = await waitFor('/kappa/.test(document.querySelector(".header .title")?.innerText || "") && !!document.querySelector(".tabs")', 8000);
  check('the space the board opened for a repo (spaces.json) opens that project\'s card', byId, await text('.header .title'));
  setWs({ id: 'wT9', label: 'ZET-1 Готово к вливанию' });
  const byTag = await waitFor('/zeta/.test(document.querySelector(".header .title")?.innerText || "")', 8000);
  check('a task\'s space (its label starts with ZET-1) opens the project of that key', byTag, await text('.header .title'));
  setWs({ id: 'wX', label: 'eta' });
  const byName = await waitFor('/eta/.test(document.querySelector(".header .title")?.innerText || "")', 8000);
  check('a space named like a project opens it', byName, await text('.header .title'));
  await key('Escape');
  setWs(null);
  await sleep(3500);
  // the bar popup asks for a project
  const [s] = await api('POST', '/api/action', { kind: 'show', slug: 'theta' });
  const shown = await waitFor('/theta/.test(document.querySelector(".header .title")?.innerText || "") && !!document.querySelector(".tabs")', 5000);
  check('the bar popup\'s «show» request opens that project\'s card in the open page', s === 200 && shown, await text('.header .title'));
  await key('Escape');
  check('esc returns to the list', !!(await evalJs('!!document.querySelector(".controls")')));
  // the request is not obeyed twice: a refresh does not bring the card back
  await refresh();
  await sleep(1200);
  check('a request is obeyed once: the next snapshot leaves the list alone', !!(await evalJs('!!document.querySelector(".controls")')));
}

// ----- the board answers badly
async function errorChecks() {
  writeFileSync(join(fixture, 'data', 'list-fails'), '');
  await refresh();
  const shown = await waitFor('/Не удалось получить проекты.*board is locked/.test(document.querySelector(".note.error")?.innerText || "")', 6000);
  check('a board that cannot be read is said so with its words, and the projects stay', shown && (await evalJs('document.querySelectorAll(".row[data-row]").length')) >= 12, await text('.note.error'));
  rmSync(join(fixture, 'data', 'list-fails'));
  writeFileSync(join(fixture, 'data', 'list-garbage'), '');
  await refresh();
  const junk = await waitFor('/не JSON/.test(document.querySelector(".note.error")?.innerText || "")', 6000);
  check('an answer that is not JSON is an error too', junk, await text('.note.error'));
  rmSync(join(fixture, 'data', 'list-garbage'));
  await refresh();
  check('…and the page recovers when the board does', await waitFor('!document.querySelector(".note.error")', 6000));
}

// ----- the settings screen and the group suggestion (fixture)
async function settingsChecks() {
  const f = check;
  const defs = (sel) => evalJs(`document.querySelectorAll(${JSON.stringify(sel)}).length`);
  const groupRows = () => evalJs('[...document.querySelectorAll(".grow-row")].map((r) => r.querySelector(".gname").textContent.replace(/\\s+/g, " ").trim())');
  await click('[data-act=settings]');
  f('«⚙ Настройки» opens the settings screen with the two settings that are left', /Настройки/.test(await text('.header .title')) && (await defs('.card[data-row-key^="s:"]')) === 2, await text('.header .title'));
  f('…none of the TASKS.md ones (freshness check, forgotten review) remain', (await defs('.card[data-row-key="s:autocheck"], .card[data-row-key="s:review_stale"]')) === 0);
  f('the agent defaults to Claude Haiku 5.5; the group suggestion is off as seeded', /● Claude Haiku 5\.5/.test(await text('.card[data-row-key="s:agent"] .badge.active')) && /● выкл/.test(await text('.card[data-row-key="s:autogroup"] .badge.active')));
  const g0 = await groupRows();
  f('the groups card lists the five starting groups with their counts', g0.length === 5 && /^▸?\s*1\. Omarchy\s+0 проектов$/.test(g0[0]) && /5\. Личное/.test(g0[4]), JSON.stringify(g0));
  await shot('settings.png');
  await click('.card[data-row-key="s:agent"] .badge[data-value=deepseek]');
  await waitFor('/DeepSeek/.test(document.querySelector(".card[data-row-key=\'s:agent\'] .badge.active")?.innerText || "")');
  f('choosing the cheap agent sticks (state.json) and shows', (await snapshot()).settings.agent === 'deepseek' && /● DeepSeek Flash/.test(await text('.card[data-row-key="s:agent"] .badge.active')));
  await click('.card[data-row-key="s:agent"] .badge[data-value=haiku]');
  await waitFor('/Haiku/.test(document.querySelector(".card[data-row-key=\'s:agent\'] .badge.active")?.innerText || "")');
  await key('ArrowRight');
  await waitFor('/DeepSeek/.test(document.querySelector(".card[data-row-key=\'s:agent\'] .badge.active")?.innerText || "")');
  f('→ turns the selected setting to its next value', (await snapshot()).settings.agent === 'deepseek' && /^s:agent$/.test(await evalJs('document.querySelector(".card.sel")?.getAttribute("data-row-key")')));
  await key('ArrowLeft');
  await waitFor('/Haiku/.test(document.querySelector(".card[data-row-key=\'s:agent\'] .badge.active")?.innerText || "")');
  f('← turns it back', (await snapshot()).settings.agent === 'haiku');

  // add, rename, move, delete a group
  await key('a');
  f('a opens the group editor with the cursor in it', await evalJs('document.activeElement?.id === "gedit"'));
  await typed('Тест');
  await key('Enter');
  f('Enter adds the group at the end, selected', (await groupRows()).at(-1)?.includes('6. Тест') && /Тест/.test(await text('.grow-row.sel')), JSON.stringify(await groupRows()));
  await key('Enter');
  f('Enter on a group renames it: the editor holds the name', await evalJs('document.activeElement?.id === "gedit" && document.activeElement.value === "Тест"'));
  await typed('2');
  await key('Enter');
  f('…the new name replaces the old one', /6\. Тест2/.test((await groupRows()).at(-1) || '') && !/6\. Тест\s/.test((await groupRows()).at(-1) || ''), JSON.stringify(await groupRows()));
  await key('[');
  const moved = await groupRows();
  f('[ moves the group up one place', /5\. Тест2/.test(moved[4]) && /6\. Личное/.test(moved[5]), JSON.stringify(moved));
  await key('x');
  f('x on an empty group asks to delete it (the group is empty)', /Группа пуста/.test(await text('.confirm')), await text('.confirm'));
  await key('n');
  f('n leaves it', (await groupRows()).length === 6 && !(await evalJs('!!document.querySelector(".confirm")')));
  await key('x');
  await key('y');
  await waitFor('document.querySelectorAll(".grow-row").length === 5');
  f('y deletes it', (await groupRows()).length === 5 && !(await groupRows()).some((r) => /Тест2/.test(r)), JSON.stringify(await groupRows()));
  await click('[data-s=gadd]');
  await typed('Омарчи');
  await key('Enter');
  await waitFor('document.querySelectorAll(".grow-row").length === 6');
  await key('Enter');
  await evalJs('(() => { const e = document.querySelector("#gedit"); e.value = "omarchy"; e.dispatchEvent(new Event("input", { bubbles: true })); })()');
  await key('Enter');
  f('a name that is taken (any case) is refused with the reason', /такая группа уже есть/.test(await text('.note.error')), await text('.note.error'));
  await key('Escape');
  await key('x');
  await key('y');
  await waitFor('document.querySelectorAll(".grow-row").length === 5');

  // «Разложить заново»: the agent sorts the projects without a group
  f('no group suggestion ran while it is off', logLines('groups.jsonl').length === 0);
  await click('[data-s=gresort]');
  const spin = await waitFor('/раскладывает проекты/.test(document.body.innerText)', 3000);
  const done = await waitFor('/Omarchy\\s+1 проект/.test(document.querySelector(".grow-row")?.innerText || "")', 8000);
  const gl = logLines('groups.jsonl');
  f('«Разложить заново» asks the agent even with the suggestion off, and shows it working', spin && done && gl.length === 1, JSON.stringify(await groupRows()));
  f('…it sees the groups with their hints and every repo of the board with its folder and the titles of its tasks',
    /^Группы:\n- Omarchy — проекты вокруг Linux-окружения Omarchy/.test(gl[0].prompt) && /Проекты \(slug \| название \| о проекте\):/.test(gl[0].prompt)
    && /\nalpha \| alpha \| .*\/repos\/alpha · задачи: Ещё один кэш; Старое дело; Три варианта; Заметка на потом/.test(gl[0].prompt) && /\nbeta \| beta \| .*\/repos\/beta\n/.test(gl[0].prompt) && /\norphan \| orphan \| задачи: Задача без репозитория/.test(gl[0].prompt), gl[0].prompt.slice(0, 700));
  const after = await groupRows();
  f('…and the counts follow: Omarchy 1, Личное 1, ИИ-инфраструктура 10 (the other repos)', /Omarchy\s+1 проект$/.test(after[0]) && /ИИ-инфраструктура\s+10 проектов$/.test(after[2]) && /Личное\s+1 проект$/.test(after[4]), JSON.stringify(after));
  const s1 = await snapshot();
  f('the guesses are marked «авто»', s1.groups.assign.alpha?.auto === true && s1.groups.assign.alpha?.g === 'Omarchy' && s1.groups.assign.gamma?.g === 'ИИ-инфраструктура' && s1.groups.on === null, JSON.stringify(s1.groups.assign));
  await shot('settings-groups.png');

  // A manual pick stays when the guesses are redone.
  await api('POST', '/api/groups', { op: 'assign', slug: 'alpha', name: 'Личное' });
  await click('[data-s=gresort]');
  await waitFor('!/раскладывает проекты/.test(document.body.innerText)', 8000);
  const gl2 = logLines('groups.jsonl');
  const s2 = await snapshot();
  f('«Разложить заново» forgets only the «авто» ones: a manual pick is not asked again and stays', gl2.length === 2 && !/\nalpha \|/.test(gl2[1].prompt.split('Проекты')[1]) && /\nbeta \|/.test(gl2[1].prompt) && s2.groups.assign.alpha?.auto === false && s2.groups.assign.alpha?.g === 'Личное', JSON.stringify(s2.groups.assign));

  // Deleting a group with projects: they move to the chosen group (as a manual pick) or to «Прочее».
  await click('[data-s=gdel][data-name="Личное"]');
  f('x on a group with projects asks where they go', /В группе 2 проекта\. Перенести в:\s*Прочее/.test(await text('.confirm')), await text('.confirm'));
  await click('[data-s=gdel-to]');
  await click('[data-s=gdel-to]');
  f('the target steps through the other groups', /Перенести в:\s*ЯНАО/.test(await text('.confirm')), await text('.confirm'));
  await click('[data-s=gdel-yes]');
  await waitFor('!(document.body.innerText.includes("Личное"))', 5000);
  const s3 = await snapshot();
  f('the projects moved to the chosen group as manual picks; the group is gone', s3.groups.assign.alpha?.g === 'ЯНАО' && s3.groups.assign.alpha?.auto === false && s3.groups.assign.beta?.g === 'ЯНАО' && s3.groups.assign.beta?.auto === false && !s3.groups.list.includes('Личное'), JSON.stringify(s3.groups));
  await click('[data-s=gdel][data-name="ИИ-инфраструктура"]');
  await click('[data-s=gdel-yes]');
  await waitFor('!(document.body.innerText.includes("ИИ-инфраструктура"))', 5000);
  const s4 = await snapshot();
  f('deleting to «Прочее» (the default) leaves the guess a guess', s4.groups.assign.gamma?.g === '' && s4.groups.assign.gamma?.auto === true, JSON.stringify(s4.groups.assign.gamma));
  await click('[data-s=gup][data-name="Агенты и herdr"]');
  await waitFor('/2\\. Агенты и herdr/.test(document.body.innerText)', 4000);
  f('a group moves up with its ↑ button', /2\. Агенты и herdr/.test((await groupRows())[1]), JSON.stringify(await groupRows()));

  // Wrong inputs
  const [b1] = await api('POST', '/api/groups', { op: 'rename', name: 'Omarchy', to: 'Прочее' });
  const [b2] = await api('POST', '/api/groups', { op: 'frobnicate' });
  const [b3] = await api('POST', '/api/groups', { op: 'move', name: 'Omarchy', delta: -1 });
  const [b4] = await api('POST', '/api/settings', { key: 'agent', value: 'gpt' });
  const [b5] = await api('POST', '/api/settings', { key: 'nope', value: 'on' });
  const [b6] = await api('POST', '/api/groups', { op: 'delete', name: 'Нет такой' });
  const [b7] = await api('POST', '/api/settings', { key: 'autocheck', value: 'on' });
  f('group and setting ops: «Прочее» is taken 400, unknown op 400, nowhere to move 400, bad value 400, bad key 400, unknown group 400, a setting that is gone 400', [b1, b2, b3, b4, b5, b6, b7].join() === '400,400,400,400,400,400,400', [b1, b2, b3, b4, b5, b6, b7].join());
  await key('Escape');
  f('esc leaves the settings: the list is back', !!(await evalJs('!!document.querySelector(".controls") && !document.querySelector(".card")')));

  // A new repo of the board gets its group guess once, by itself, when the suggestion is on.
  await api('POST', '/api/settings', { key: 'autogroup', value: 'on' });
  await sleep(500);
  const asked0 = logLines('groups.jsonl').length;
  const reposFile = join(fixture, 'board', 'repos.json');
  const repos = JSON.parse(readFileSync(reposFile, 'utf8'));
  repos.delta = join(fixture, 'repos/delta');
  writeFileSync(reposFile, JSON.stringify(repos));
  await refresh();
  const grouped = await waitFor('(async () => { const s = await fetch("/api/snapshot").then(r => r.json()); return s.groups.assign.delta?.g === "Новая область"; })()', 12000);
  const gl3 = logLines('groups.jsonl');
  const s5 = await snapshot();
  f('a new repo on the board gets its group guess once: one call that asks only about it', grouped && gl3.length > asked0 && /\ndelta \| delta \| .*\/repos\/delta\n?/.test(gl3.at(-1).prompt) && !/\nalpha \|/.test(gl3.at(-1).prompt.split('Проекты')[1]), (gl3.at(-1)?.prompt || '').slice(-200));
  f('…a group the agent named is added at the end and the project sits in it as a guess', s5.groups.list.at(-1) === 'Новая область' && s5.groups.assign.delta?.auto === true, JSON.stringify([s5.groups.list, s5.groups.assign.delta]));
}

// ----- live server: the screens are looked at, nothing is changed
async function screensSmoke() {
  await click('[data-act=settings]');
  check('settings: a card per setting and the groups card', (await evalJs('document.querySelectorAll(".card[data-row-key^=\'s:\']").length')) >= 2 && /Группы проектов/.test(await text('.scroll')));
  check('settings: the cheap agent shows Haiku 5.5 as an option', /Claude Haiku 5\.5/.test(await text('.card[data-row-key="s:agent"]')));
  await shot('settings.png');
  await key('Escape');
  check('esc leaves the settings', !!(await evalJs('!!document.querySelector(".controls")')));
}

async function main() {
  if (fixture) startSocket();
  await waitForChrome();
  let target = await (await fetch(`http://127.0.0.1:${port}/json/new?${base}/`, { method: 'PUT' })).json().catch(() => null);
  if (!target) target = await (await fetch(`http://127.0.0.1:${port}/json/new?${base}/`)).json();
  ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((r, e) => { ws.onopen = r; ws.onerror = e; });
  ws.onmessage = (m) => { const d = JSON.parse(m.data); if (d.id && pending.has(d.id)) { pending.get(d.id)(d); pending.delete(d.id); } };
  await send('Page.enable');
  await send('Runtime.enable');
  // Wait for the first snapshot to arrive over SSE.
  for (let i = 0; i < 50 && !(await evalJs('document.querySelectorAll(".row[data-row], .header .back").length > 0')); i++) await sleep(200);
  // The page follows the focused herdr workspace and may open a project at once.
  if (await evalJs('!!document.querySelector(".header .back")')) { console.log('note: opened on the active workspace\'s project'); await key('Escape'); await key('Escape'); }

  const rows = await evalJs('document.querySelectorAll(".row[data-row]").length');
  check('list has project rows', rows > 1, `${rows} rows`);
  const title = await evalJs('document.querySelector(".header .title")?.textContent');
  check('header names the planner and the count', /Планировщик проектов · \d+ проект/.test(title || ''), title);
  const overflow = await evalJs('document.scrollingElement.scrollWidth - window.innerWidth');
  check('page does not scroll horizontally', overflow <= 0, `overflow ${overflow}px`);
  const stamp = await evalJs('(() => { const r = document.querySelector(".stamp").getBoundingClientRect(); return [Math.round(r.right), window.innerWidth]; })()');
  check('refresh stamp fits the viewport', stamp && stamp[0] <= stamp[1], JSON.stringify(stamp));
  const cut = await evalJs('(() => { const e = document.querySelector(".stamp"); return [e.scrollWidth, e.clientWidth, e.textContent]; })()');
  check('refresh stamp text is not cut', cut && cut[0] <= cut[1] + 1, JSON.stringify(cut));
  const vars = await evalJs('getComputedStyle(document.documentElement).getPropertyValue("--bg").trim()');
  check('theme variables applied', /^#[0-9a-f]{6}$/i.test(vars) && vars !== '#101419', vars);
  const font = await evalJs('getComputedStyle(document.body).fontFamily');
  check('ui font from herdr-gpui config', !!font, font);
  // The bar icon's endpoint agrees with the list it summarises.
  const sum = await evalJs(`Promise.all([fetch("/api/summary").then(r => r.json()), fetch("/api/snapshot").then(r => r.json())]).then(([m, s]) => {
    const add = (f) => s.projects.reduce((n, p) => n + f(p), 0);
    return { m, want: { waiting: add((p) => p.counts.needs + p.counts.ready + p.counts.failed), review: add((p) => p.counts.review), working: add((p) => p.counts.working), projects: s.projects.length } };
  })`);
  check('/api/summary: waiting (needs + ready + failed), review, working and projects match the list',
    ['waiting', 'review', 'working', 'projects'].every((k) => sum.m[k] === sum.want[k]), JSON.stringify(sum));
  if (fixture) await boardChecks();
  await shot('list.png');

  await key('ArrowDown');
  await key('ArrowDown');
  const hl = await evalJs('document.querySelector(".row.hl")?.getAttribute("data-row") || document.querySelector(".row.hl")?.getAttribute("data-group")');
  check('arrow keys highlight a row', !!hl, hl);
  const hint = await evalJs('document.querySelector(".row.hl .hint")?.textContent');
  check('highlighted row shows its keys', !!hint, hint);
  await shot('highlight.png');

  await key('t');
  const picker = await evalJs('document.querySelector(".dialog h2")?.textContent');
  check('T opens the group picker', /Группа для/.test(picker || ''), picker);
  await key('Escape');
  check('Escape closes the picker', !(await evalJs('!!document.querySelector(".dialog")')));

  await key('s');
  const sort = await evalJs('document.querySelector("[data-act=sort]")?.textContent');
  check('S toggles the sort', /внимание/.test(sort || ''), sort);
  await key('s');
  check('S toggles it back', /новые/.test(await evalJs('document.querySelector("[data-act=sort]")?.textContent') || ''));
  check('the list has «все проекты» next to the sort and group switches', /все проекты\s*U/i.test(await evalJs('document.querySelector("[data-act=unfocus]")?.textContent') || ''));
  if (fixture) await listChecks();
  await key('Escape');
  const word = (await evalJs('document.querySelector(".row[data-row]")?.getAttribute("data-row") || ""') || '').slice(0, 4);
  await key('/');
  check('/ focuses the filter', await evalJs('document.activeElement?.id === "filter"'));
  await send('Input.insertText', { text: word });
  await sleep(150);
  const shown = await evalJs('document.querySelectorAll(".row[data-row]").length');
  check('typing filters the list', shown > 0 && shown <= rows, `${shown} of ${rows}`);
  await key('ArrowDown');
  check('↓ leaves the filter and keeps the text', await evalJs('document.activeElement?.id !== "filter" && document.querySelector("#filter").value === ' + JSON.stringify(word)));
  if (fixture) {
    // The card of alpha, whatever else the filter found.
    await evalJs('(() => { const f = document.querySelector("#filter"); f.value = "alpha"; f.dispatchEvent(new Event("input", { bubbles: true })); })()');
    await key('ArrowDown');
  }
  await key('Enter');
  await waitFor('!!document.querySelector(".tabs")');
  const cardOpen = await evalJs('!!document.querySelector(".tabs") && !!document.querySelector(".header .back")');
  check('Enter opens the project card', cardOpen, cardOpen ? '' : (await evalJs('document.body.innerText.slice(0, 300)') || '').replace(/\s+/g, ' '));
  await sleep(500);
  await shot('detail.png');
  if (fixture) await cardChecks(); else await cardSmoke();
  await key('Escape');
  check('Escape returns to the list with the project highlighted', await evalJs('!!document.querySelector(".row.hl[data-row]")'));
  await evalJs('(() => { const f = document.querySelector("#filter"); f.value = ""; f.dispatchEvent(new Event("input", { bubbles: true })); })()');
  if (fixture) { await followChecks(); await errorChecks(); await settingsChecks(); } else await screensSmoke();

  ws.close();
}

main().catch((e) => { console.error('ERROR', e); failed++; }).finally(() => {
  chrome.kill();
  sockServer?.close();
  try { rmSync(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 }); } catch {}
  process.exit(failed ? 1 : 0);
});
