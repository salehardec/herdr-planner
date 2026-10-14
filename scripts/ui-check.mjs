#!/usr/bin/env node
// Checks of the page in headless Chromium over the DevTools protocol.
// No dependencies (Node 22+ has WebSocket). Usage, with the server running:
//   node scripts/ui-check.mjs [http://127.0.0.1:7117] [screenshot-dir]
// Exits 1 when a check fails. Needs `chromium` (or $CHROMIUM) on the PATH.
//
// Against a live server only the list and a read-only look at the first
// project's card are checked. With FIXTURE=DIR (a server started on the world
// of scripts/fixture.mjs; `scripts/run-checks.sh` does all of it) the card is
// checked in full: threads, git lines, badges, worktree, tasks, «Решить»,
// «Не решать», questions — with the fake binaries' logs as the witness.

import { spawn } from 'node:child_process';
import http from 'node:http';
import { mkdtempSync, writeFileSync, rmSync, readFileSync, existsSync } from 'node:fs';
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
const ids = () => evalJs('[...document.querySelectorAll(".list .row")].map((r) => r.getAttribute("data-id"))');
const logLines = (name) => {
  const f = join(fixture, 'log', name);
  return existsSync(f) ? readFileSync(f, 'utf8').split('\n').filter(Boolean).map((l) => JSON.parse(l)) : [];
};
const api = (method, path, body) => evalJs(`fetch(${JSON.stringify(path)}, { method: ${JSON.stringify(method)}, headers: { 'content-type': 'application/json' }, body: ${body === undefined ? 'undefined' : JSON.stringify(JSON.stringify(body))} }).then(async (r) => [r.status, await r.json().catch(() => null)])`);
const click = (sel) => evalJs(`(() => { const e = document.querySelector(${JSON.stringify(sel)}); if (e) e.click(); return !!e; })()`);
const note = () => text('.note');

// The card of a live project, read-only: what is there is there.
async function cardSmoke() {
  check('card has tabs and a filter', await evalJs('!!document.querySelector(".tabs") && !!document.querySelector("#cfilter")'));
  const overflow = await evalJs('document.scrollingElement.scrollWidth - window.innerWidth');
  check('card does not scroll horizontally', overflow <= 0, `overflow ${overflow}px`);
  await key('2');
  check('2 opens «Сделано»', /Сделано/.test(await text('.tab.active')));
  await key('1');
  check('1 opens «Сделать»', /Сделать/.test(await text('.tab.active')));
}

async function cardChecks() {
  const f = (name, ok, extra) => check(name, ok, extra);
  const title = await text('.header .title');
  f('card header names the project', /Alpha/.test(title), title);
  const kv = await text('.kv');
  f('goal and both repositories are shown, the remote one with its machine', /Проект для проверки карточки/.test(kv) && /@mac/.test(kv), kv.replace(/\s+/g, ' '));
  f('progress counts threads and tasks', /5\s+из 13 сделано\s+·\s+8 тредов/.test(await text('.progress')), await text('.progress'));
  const order = await ids();
  f('open threads first (failed start, review, working, idle), then tasks', JSON.stringify(order) === JSON.stringify(['t-0007', 't-0003', 't-0005', 't-0002', 'k1', 'k2', 'k3', 'k4']), JSON.stringify(order));
  const row = (id) => text(`.row[data-id="${id}"]`);
  const failed = await row('t-0007');
  f('a failed start says so, in red, with the error', /сбой запуска/.test(failed) && /timed out/.test(failed) && await evalJs('!!document.querySelector(".row[data-id=t-0007] .detail .error")'), failed.replace(/\s+/g, ' '));

  // git lines arrive in the background
  const git = await waitFor('/не закоммичено: 2/.test(document.querySelector(".row[data-id=t-0005] .detail")?.innerText || "") && /PR #12 открыт/.test(document.querySelector(".row[data-id=t-0003] .detail")?.innerText || "")');
  const w5 = (await row('t-0005')).replace(/\s+/g, ' ');
  f('git line: branch, commits, files with +/−, uncommitted', git && /⎇ t-0005-1/.test(w5) && /1 коммит/.test(w5) && /2 файла \+2 −0/.test(w5) && /● не закоммичено: 2/.test(w5), w5);
  const w3 = (await row('t-0003')).replace(/\s+/g, ' ');
  f('git line of a clean branch with an open PR', /1 коммит/.test(w3) && /всё закоммичено/.test(w3) && /PR #12 открыт/.test(w3), w3);
  f('a tab thread has no worktree', /без ворктри/.test(await row('t-0002')));
  f('herdr states in words', /ревью · пейн закрыт/.test(w3.replace(/\s+/g, ' ')) && /в работе ~40%/.test(w5), w3);
  const badge = (id) => evalJs(`[...document.querySelectorAll('.row[data-id="${id}"] .mdl')].map((b) => b.textContent + ':' + b.className.replace('mdl ', '')).join(',')`);
  f('badges from the profile: Sonnet 5.5, Haiku 5.5, pi', (await badge('t-0005')) === 'Sonnet 5.5:fam-sonnet' && (await badge('t-0003')) === 'Haiku 5.5:fam-haiku' && (await badge('t-0002')) === 'pi:fam-other', `${await badge('t-0005')} | ${await badge('t-0003')} | ${await badge('t-0002')}`);
  const overflow = await evalJs('document.scrollingElement.scrollWidth - window.innerWidth');
  f('card does not scroll horizontally', overflow <= 0, `overflow ${overflow}px`);
  await shot('card-todo.png');

  // keys: ↑ ↓ enter w
  await key('ArrowDown'); await key('ArrowDown'); await key('ArrowDown');
  f('↓ highlights threads row by row', (await evalJs('document.querySelector(".row.hl")?.getAttribute("data-id")')) === 't-0005');
  f('the highlighted thread shows its keys', /Раскрыть/.test(await text('.row.hl .hint')) && /Ворктри/.test(await text('.row.hl .hint')));
  await key('Enter');
  const opened = await waitFor('/Изменения против main/.test(document.querySelector(".outcome")?.innerText || "")');
  const block = await text('.outcome[data-thread="t-0005"]');
  f('Enter opens the thread: branch, base, worktree, launch, model', opened && /hp\/alpha\/t-0005-1/.test(block) && /База\s+main/.test(block) && /claude · профиль claude-sonnet · claude-sonnet-5-5 · effort high/.test(block) && /Sonnet 5\.5\s*по профилю/.test(block), block.replace(/\s+/g, ' ').slice(0, 240));
  f('…its diff, last commits and files not committed', /src\/a\.rs \| 1 \+/.test(block) && /[0-9a-f]{7}\s+Добавить a и b/.test(block) && /\?\? notes\.txt/.test(block) && /M src\/a\.rs/.test(block));
  await shot('card-expanded.png');
  const before = logLines('herdr.jsonl').length;
  await key('w');
  await waitFor(`1`, 50);
  const opens = await waitFor('/открыт в herdr/.test(document.querySelector(".note")?.innerText || "")');
  const wl = logLines('herdr.jsonl');
  f('W opens the worktree through herdr', opens && wl.length === before + 1 && wl.at(-1).argv.join(' ').includes('worktree open --path') && wl.at(-1).argv.join(' ').endsWith('worktrees/t-0005 --focus'), JSON.stringify(wl.at(-1)));
  await key('Enter');
  f('Enter closes the block', !(await evalJs('!!document.querySelector(".outcome[data-thread=t-0005]")')));
  await key('ArrowDown');
  await key('w');
  await waitFor('/нет/.test(document.querySelector(".note")?.innerText || "")');
  f('W on a thread without a worktree says so and runs nothing', /у треда нет/.test(await note()) && logLines('herdr.jsonl').length === before + 1, await note());
  await key('Enter');
  f('a thread without a worktree says so in its block', /У треда нет своего ворктри/.test(await text('.outcome[data-thread="t-0002"]')));
  await key('Enter');
  f('Enter again closes the block', !(await evalJs('!!document.querySelector(".outcome[data-thread=t-0002]")')));
  await click('.row[data-id="t-0005"]');
  f('a click opens a thread too', await evalJs('!!document.querySelector(".outcome[data-thread=t-0005]")'));
  await click('.row[data-id="t-0005"]');
  f('…and closes it', !(await evalJs('!!document.querySelector(".outcome[data-thread=t-0005]")')));

  // «Сделано»
  await key('2');
  f('2 opens «Сделано» with its count', /Сделано\s*5/.test(await text('.tab.active')));
  const done = await ids();
  f('finished threads newest first, then done tasks', JSON.stringify(done) === JSON.stringify(['t-0008', 't-0006', 't-0004', 't-0001', 'k5']), JSON.stringify(done));
  await waitFor('document.querySelector(".row[data-id=t-0001] .mdl.fam-opus")');
  f('badge from the commits of a merged PR (squash commit)', (await badge('t-0001')) === 'Opus 5.5:fam-opus', await badge('t-0001'));
  f('badge from the branch trailers of a finished thread', (await badge('t-0008')) === 'Haiku 5.5:fam-haiku', await badge('t-0008'));
  f('no trailers: the profile, quietly', (await badge('t-0004')) === 'DS Flash:quiet', await badge('t-0004'));
  f('«итог» when there is a report, «без отчёта» when not', /итог/.test(await row('t-0001')) && /без отчёта/.test(await row('t-0008')));
  f('a closed failed start is quiet: «не запустился», no second line', /✕ не запустился/.test(await row('t-0006')) && !(await evalJs('!!document.querySelector(".row[data-id=t-0006] .detail")')) && !(await evalJs('!!document.querySelector(".row[data-id=t-0006] .error")')));
  await click('.row[data-id="t-0001"]');
  const merged = await text('.outcome[data-thread="t-0001"]');
  f('a finished thread opens with its outcome, PR and the source of its models', /Экспорт отчёта готов, PR слит/.test(merged) && /PR #7 слит/.test(merged) && /по коммитам \(слитый коммит\)/.test(merged) && /PR слит/.test(merged) && /закрыт|сделан/.test(merged) && /удалён/.test(merged), merged.replace(/\s+/g, ' ').slice(0, 300));
  await shot('card-done.png');
  await click('.row[data-id="t-0001"]');

  // the card's filter
  await key('/');
  f('/ focuses the card filter', await evalJs('document.activeElement?.id === "cfilter"'));
  await send('Input.insertText', { text: 'haiku' });
  await sleep(200);
  f('the filter finds threads by model, in both tabs', /Сделать\s*1/.test(await text('.tabs')) && /Сделано\s*1/.test(await text('.tabs')), await text('.tabs'));
  f('…and counts what it shows', /2 из 13/.test(await text('.controls')), await text('.controls'));
  await key('ArrowDown');
  f('↓ leaves the filter and keeps the text', await evalJs('document.activeElement?.id !== "cfilter" && document.querySelector("#cfilter").value === "haiku"'));
  await click('[data-c=filter-clear]');
  await key('/');
  await send('Input.insertText', { text: 'сбой запуска' });
  await sleep(200);
  f('«сбой запуска» finds the failed starts', /Сделать\s*1/.test(await text('.tabs')) && /Сделано\s*1/.test(await text('.tabs')), await text('.tabs'));
  await send('Input.insertText', { text: ' zzz' });
  await sleep(200);
  f('typing goes on at the caret after each re-render', (await evalJs('document.querySelector("#cfilter").value')) === 'сбой запуска zzz');
  await key('Escape');
  f('nothing found says so', /Ничего не найдено по «сбой запуска zzz»/.test(await text('.list')), await text('.list'));
  await click('[data-c=filter-clear]');
  await key('1');

  // tasks
  const k1 = 'k1';
  await click(`.row[data-id="${k1}"]`);
  const panel = await text('.panel');
  f('a task picks into a panel with its notes and four actions', /Починить экспорт отчёта/.test(panel) && /Смотреть export\.txt/.test(panel) && /Решить/.test(panel) && /профиль/.test(panel) && /Задать вопрос/.test(panel) && /Не решать/.test(panel), panel.replace(/\s+/g, ' ').slice(0, 200));
  await shot('card-panel.png');
  await key('?');
  f('? opens the question field', await evalJs('document.activeElement?.id === "cask"'));
  await send('Input.insertText', { text: 'Что нужно сделать?' });
  await key('Enter');
  f('a question shows the agent thinking', await waitFor('/Haiku 5\\.5 думает/.test(document.querySelector(".qa")?.innerText || "")', 4000), await text('.qa'));
  const answered = await waitFor('document.querySelector(".qa .md strong")?.textContent === "готово"', 10000);
  f('the answer arrives and is rendered as markdown', answered && (await evalJs('document.querySelectorAll(".qa .md li").length')) === 2 && (await evalJs('!!document.querySelector(".qa .md code")')), await text('.qa'));
  f('markup in an answer is escaped, never run', !(await evalJs('!!document.querySelector(".qa script")')) && /<script>alert\(1\)<\/script>/.test(await text('.qa')));
  const cl = logLines('claude.jsonl');
  const ask = cl.at(-1) || { argv: [], prompt: '' };
  const dirs = ask.argv.slice(ask.argv.indexOf('--add-dir') + 1);
  f('the agent is read-only Haiku in the project folder with the local repository', cl.length === 1 && ask.argv.includes('claude-haiku-5-5') && ask.argv.includes('Read,Grep,Glob') && ask.argv.includes('--disallowedTools') && ask.cwd.endsWith('/root/alpha') && dirs.length === 1 && dirs[0].endsWith('/repos/alpha'), JSON.stringify({ n: cl.length, cwd: ask.cwd, dirs }));
  f('its question names the task, its notes, both repositories and the question', /«Починить экспорт отчёта \(claude-sonnet\)»/.test(ask.prompt) && /Смотреть export\.txt/.test(ask.prompt) && /\/srv\/alpha-mac \(на машине mac, здесь недоступен\)/.test(ask.prompt) && /Вопрос: Что нужно сделать\?/.test(ask.prompt), ask.prompt);
  await click('[data-c=profile-toggle]');
  await waitFor('/pi@mac/.test(document.querySelector(".options")?.innerText || "")');
  const opts = await text('.options');
  f('the profile menu lists the coordinator’s pick and the assignable profiles', /на выбор координатора/.test(opts) && /claude-opus/.test(opts) && /deepseek-flash/.test(opts) && /pi@mac/.test(opts), opts.replace(/\s+/g, ' '));
  await click('[data-c=profile][data-id=claude-opus]');
  f('a chosen profile shows on the button', /профиль: claude-opus/.test(await text('.panel')), await text('.panel'));
  await click('[data-c=solve]');
  f('Решить on a project that is not open opens it first', await waitFor('/Открываю проект/.test(document.querySelector(".panel")?.innerText || "")', 3000), await text('.panel'));
  const sentOk = await waitFor('/Задача отправлена координатору/.test(document.querySelector(".note")?.innerText || "")', 12000);
  f('…then the coordinator takes the task', sentOk, await note());
  const pr = logLines('prompts.jsonl');
  f('first prompt was refused (not open), `open` ran once, the retry went through', pr.length === 2 && pr[0].opened === false && pr[1].opened === true && logLines('opens.jsonl').length === 1, JSON.stringify(pr.map((p) => p.opened)));
  const sentText = pr.at(-1)?.text || '';
  f('the sentence names the task, the profile and the answered question', /Реши задачу из TASKS\.md: «Починить экспорт отчёта \(claude-sonnet\)»/.test(sentText) && /с профилем claude-opus/.test(sentText) && /В: Что нужно сделать\? — О: Ответ из теста/.test(sentText), sentText);
  f('the pick is gone and the task is marked as sent', !(await evalJs('!!document.querySelector(".panel")')) && /отправлено координатору/.test(await row('k1')), await row('k1'));
  const state = await evalJs('fetch("/api/project/alpha").then(r => r.json()).then(d => JSON.stringify([d.sent, Object.keys(d.qa)]))');
  f('the mark and the answers are kept in the state by task text', /отправлено|true/.test(state) && /Починить экспорт отчёта/.test(state), state);

  // «Не решать»
  await click('.row[data-id="k2"]');
  await key('x');
  f('x asks «Убрать из бэклога?»', /Убрать из бэклога\?/.test(await text('.panel')) && /y.*n/s.test(await text('.dock')), await text('.panel'));
  await key('n');
  f('n keeps the task: nothing is sent', !/Убрать из бэклога/.test(await text('.panel')) && logLines('prompts.jsonl').length === 2);
  await key('x');
  await key('y');
  const removed = await waitFor('/Просьба убрать задачу отправлена/.test(document.querySelector(".note")?.innerText || "")', 8000);
  const rp = logLines('prompts.jsonl');
  f('y asks the coordinator to remove the line (TASKS.md is not edited here)', removed && rp.length === 3 && /отказался от задачи «Добавить тёмную тему»: удали её из TASKS\.md/.test(rp.at(-1).text), rp.at(-1)?.text);
  f('the task shows «просьба убрать отправлена»', /просьба убрать отправлена/.test(await row('k2')), await row('k2'));
  await click('.row[data-id="k4"]');
  f('a task a thread works on has no «Не решать»', !/Не решать/.test(await text('.panel')) && /Решить/.test(await text('.panel')), await text('.panel'));
  await key('x');
  f('x on it says a thread leads it', /ведёт тред/.test(await note()) && !/Убрать из бэклога/.test(await text('.panel')), await note());
  await key('Escape');
  f('Escape drops the pick', !(await evalJs('!!document.querySelector(".panel")')));
  // The same shortcuts on a Russian layout: the key is told by its place (ч on X, т on N).
  await click('.row[data-id="k3"]');
  await key('ч', undefined, 'KeyX');
  f('shortcuts work on a Russian layout (ч = x)', /Убрать из бэклога\?/.test(await text('.panel')), await text('.panel'));
  await key('т', undefined, 'KeyN');
  f('…and т = n cancels', !/Убрать из бэклога/.test(await text('.panel')) && logLines('prompts.jsonl').length === 3);
  await key('Escape');

  // a coordinator that refuses
  writeFileSync(join(fixture, 'data', 'refuse-alpha'), '');
  await click('.row[data-id="k3"]');
  await click('[data-c=solve]');
  await waitFor('/Координатор не принял задачу/.test(document.querySelector(".note")?.innerText || "")', 8000);
  f('a refusal is shown and the task stays picked', /Координатор не принял задачу: .*refuses/.test(await note()) && (await evalJs('!!document.querySelector(".panel")')) && !/отправлено координатору/.test(await row('k3')), await note());
  rmSync(join(fixture, 'data', 'refuse-alpha'));
  await key('Escape');

  // other sites cannot reach the server through the browser
  const raw = (headers, method = 'GET', path = '/api/snapshot') => new Promise((resolve) => {
    const u = new URL(base);
    const r = http.request({ host: u.hostname, port: u.port, path, method, headers: { 'content-type': 'application/json', ...headers } }, (res) => { res.resume(); res.on('end', () => resolve(res.statusCode)); });
    r.on('error', () => resolve(0));
    r.end(method === 'POST' ? '{}' : undefined);
  });
  const hostOk = await raw({}), hostBad = await raw({ host: 'evil.example' }), originBad = await raw({ origin: 'https://evil.example' }, 'POST', '/api/refresh'), originOk = await raw({ origin: base }, 'POST', '/api/refresh');
  f('a foreign Host (DNS rebinding) and a foreign Origin (cross-site) are refused', [hostOk, hostBad, originBad, originOk].join() === '200,403,403,200', [hostOk, hostBad, originBad, originOk].join());

  // wrong inputs to the API
  const solveEnc = '/api/project/alpha/solve';
  const [s1] = await api('POST', solveEnc, { task: 'Нет такой задачи' });
  const [s2] = await api('POST', solveEnc, { task: 'Готовая задача (2026-10-01)' });
  const [s3] = await api('POST', solveEnc, { task: 'Добавить тёмную тему', profile: '-bad profile' });
  const [s4] = await api('POST', '/api/project/nope/solve', { task: 'x' });
  const [s5] = await api('POST', '/api/project/.hidden/solve', { task: 'x' });
  f('solve: unknown task 404, done task 404, bad profile 400, unknown project 404, bad slug 400', [s1, s2, s3, s4, s5].join() === '404,404,400,404,400', [s1, s2, s3, s4, s5].join());
  const [r1] = await api('POST', '/api/project/alpha/remove', { task: 'Миграция базы (claude-opus) · t-0005' });
  const [r2] = await api('POST', '/api/project/alpha/remove', { task: '' });
  f('remove: a thread-led task 409, no task 400', [r1, r2].join() === '409,400', [r1, r2].join());
  const [a1] = await api('POST', '/api/project/alpha/ask', { task: 'Добавить тёмную тему', question: '   ' });
  const [a2] = await api('POST', '/api/project/alpha/ask', { task: 'Нет такой', question: 'что?' });
  f('ask: empty question 400, unknown task 404', [a1, a2].join() === '400,404', [a1, a2].join());
  const [t1] = await api('GET', '/api/project/alpha/thread/t-9999');
  const [t2] = await api('POST', '/api/project/alpha/thread/t-0002/worktree', {});
  const [t3] = await api('POST', '/api/project/alpha/thread/t-0001/worktree', {});
  const [t4] = await api('GET', '/api/project/.hidden/thread/t-1');
  f('threads: unknown 404, tab thread 502, removed worktree 502, bad slug 400', [t1, t2, t3, t4].join() === '404,502,502,400', [t1, t2, t3, t4].join());
  f('wrong inputs sent nothing more', logLines('prompts.jsonl').length === 4 && logLines('claude.jsonl').length === 1 && logLines('herdr.jsonl').length === 1, `${logLines('prompts.jsonl').length} prompts`);

}

async function main() {
  await waitForChrome();
  let target = await (await fetch(`http://127.0.0.1:${port}/json/new?${base}/`, { method: 'PUT' })).json().catch(() => null);
  if (!target) target = await (await fetch(`http://127.0.0.1:${port}/json/new?${base}/`)).json();
  ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((r, e) => { ws.onopen = r; ws.onerror = e; });
  ws.onmessage = (m) => { const d = JSON.parse(m.data); if (d.id && pending.has(d.id)) { pending.get(d.id)(d); pending.delete(d.id); } };
  await send('Page.enable');
  await send('Runtime.enable');
  // Wait for the first snapshot to arrive over SSE.
  for (let i = 0; i < 50 && !(await evalJs('document.querySelectorAll(".row[data-row]:not(.newrow), .header .back").length > 0')); i++) await sleep(200);
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

  await key('2');
  const chip = await evalJs('document.querySelector(".badge.active")?.textContent');
  check('digit keys switch chips', /Ревью/.test(chip || ''), chip);
  await key('s');
  const sort = await evalJs('document.querySelector("[data-act=sort]")?.textContent');
  check('S toggles the sort', /внимание/.test(sort || ''), sort);
  await key('Escape');
  check('Escape resets the chip', /Все/.test(await evalJs('document.querySelector(".badge.active")?.textContent') || ''));

  const word = (await evalJs('document.querySelector(".row[data-row]:not(.newrow)")?.getAttribute("data-row") || ""') || '').slice(0, 4);
  await key('/');
  check('/ focuses the filter', await evalJs('document.activeElement?.id === "filter"'));
  await send('Input.insertText', { text: word });
  await sleep(150);
  if (process.env.DEBUG) console.log('debug', await evalJs('JSON.stringify({v: document.querySelector("#filter")?.value, active: document.activeElement?.id, note: document.querySelector(".note")?.textContent, rows: document.querySelectorAll(".row").length})'));
  const shown = await evalJs('document.querySelectorAll(".row[data-row]:not(.newrow)").length');
  check('typing filters the list', shown > 0 && shown < rows, `${shown} of ${rows}`);
  await key('ArrowDown');
  check('↓ leaves the filter and keeps the text', await evalJs('document.activeElement?.id !== "filter" && document.querySelector("#filter").value === ' + JSON.stringify(word)));
  // The highlight may already sit on the one project the filter left; ↓ then moved on to «+ Новый проект».
  if ((await evalJs('document.querySelector(".row.hl")?.getAttribute("data-row")')) === '+new') await key('ArrowUp');
  if (process.env.DEBUG) console.log('before Enter', await evalJs('JSON.stringify({hl: document.querySelector(".row.hl")?.getAttribute("data-row"), active: document.activeElement?.tagName, rows: document.querySelectorAll(".row").length})'));
  await key('Enter');
  await waitFor('!!document.querySelector(".tabs")');
  const cardOpen = await evalJs('!!document.querySelector(".tabs") && !!document.querySelector(".header .back")');
  check('Enter opens the project card', cardOpen, cardOpen ? '' : (await evalJs('document.body.innerText.slice(0, 300)') || '').replace(/\s+/g, ' '));
  await sleep(800);
  await shot('detail.png');
  if (fixture) await cardChecks(); else await cardSmoke();
  await key('Escape');
  check('Escape returns to the list with the project highlighted', await evalJs('!!document.querySelector(".row.hl[data-row]")'));

  ws.close();
}

main().catch((e) => { console.error('ERROR', e); failed++; }).finally(() => {
  chrome.kill();
  rmSync(profile, { recursive: true, force: true });
  process.exit(failed ? 1 : 0);
});
