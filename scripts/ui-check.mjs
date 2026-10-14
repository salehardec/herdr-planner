#!/usr/bin/env node
// Checks of the page in headless Chromium over the DevTools protocol.
// No dependencies (Node 22+ has WebSocket). Usage, with the server running:
//   node scripts/ui-check.mjs [http://127.0.0.1:7117] [screenshot-dir]
// Exits 1 when a check fails. Needs `chromium` (or $CHROMIUM) on the PATH.
//
// Against a live server only the list and a read-only look at the first
// project's card, the settings screen and the new-project form (nothing is changed
// or created) are checked. With FIXTURE=DIR (a server started on the world
// of scripts/fixture.mjs; `scripts/run-checks.sh` does all of it) the card is
// checked in full: threads, git lines, badges, worktree, tasks, «Решить»,
// «Не решать», questions — with the fake binaries' logs as the witness.

import { spawn, execFileSync } from 'node:child_process';
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

const tasksMd = () => join(fixture, 'root/alpha/TASKS.md');
const tasksAtStart = fixture ? readFileSync(tasksMd(), 'utf8') : '';
const gitIn = (cwd, ...args) => execFileSync('git', ['-C', cwd, ...args], { stdio: 'pipe', env: { ...process.env, GIT_AUTHOR_NAME: 'T', GIT_AUTHOR_EMAIL: 't@t', GIT_COMMITTER_NAME: 'T', GIT_COMMITTER_EMAIL: 't@t' } });
const typed = async (s) => { await send('Input.insertText', { text: s }); await sleep(150); };
const snapshot = () => evalJs('fetch("/api/snapshot").then(r => r.json())');

async function cardChecks() {
  const f = (name, ok, extra) => check(name, ok, extra);
  // The backlog check starts by itself when the card opens and shows «…» while it runs.
  const row0 = (id) => text(`.row[data-id="${id}"]`);
  f('tasks being checked show «…»; a person\'s task and a thread-led one are not asked', await waitFor('/…/.test(document.querySelector(".row[data-id=k1]")?.innerText || "") && /…/.test(document.querySelector(".row[data-id=k2]")?.innerText || "")', 2500) && !/…/.test(await row0('k3')) && !/…/.test(await row0('k4')), `${await row0('k1')} | ${await row0('k3')}`);
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
  const badged = await waitFor('/◐ частично/.test(document.querySelector(".row[data-id=k1]")?.innerText || "") && /⊘ сделано/.test(document.querySelector(".row[data-id=k2]")?.innerText || "")', 8000);
  f('verdicts: «◐ частично» and «⊘ сделано» on the tasks that are not actual; none on the others', badged && !/[◐⊘]/.test(await row('k3')) && !/[◐⊘]/.test(await row('k4')) && !/…/.test(await row('k1')), `${await row('k1')} | ${await row('k2')}`);
  const fl = logLines('fresh.jsonl');
  const fa = fl[0] || { argv: [], prompt: '', cwd: '' };
  f('one agent call asked about the two open tasks of nobody or of a profile, with their notes', fl.length === 1 && /\n1\. Починить экспорт отчёта \(claude-sonnet\)\n\s+заметка: Заметки: экспорт падает на пустом файле\.\n\s+заметка: Смотреть export\.txt\.\n2\. Добавить тёмную тему/.test(fa.prompt) && !/Выпустить ключ|Миграция базы|Готовая задача/.test(fa.prompt), fa.prompt);
  f('…naming the repository, the commit of main and the log', /Репозиторий проекта: .*\/repos\/alpha\nКоммит main: [0-9a-f]{10}\n/.test(fa.prompt) && /Экспорт отчёта \(#7\)/.test(fa.prompt), fa.prompt.slice(0, 300));
  const fdirs = fa.argv.slice(fa.argv.indexOf('--add-dir') + 1);
  f('the agent is read-only, in the repository, with the project folder; «сделано» needs a file as proof', fa.argv.includes('claude-haiku-5-5') && fa.argv.includes('Read,Grep,Glob') && fa.argv.includes('--disallowedTools') && fa.cwd.endsWith('/repos/alpha') && fdirs.length === 1 && fdirs[0].endsWith('/root/alpha') && fa.argv.some((a) => /ТОЛЬКО с доказательством в коде/.test(a)), JSON.stringify({ cwd: fa.cwd, fdirs }));
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

  await freshChecks();

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
  f('wrong inputs sent nothing more', logLines('prompts.jsonl').length === 5 && logLines('claude.jsonl').length === 1 && logLines('herdr.jsonl').length === 1, `${logLines('prompts.jsonl').length} prompts`);
  f('TASKS.md was never edited: not by the checks, not by the actions', readFileSync(tasksMd(), 'utf8') === tasksAtStart);

}

// What «◐ частично» / «⊘ сделано» do: the reason, «Убрать» (asks the coordinator), «Оставить» (hides
// until main moves), a new commit hides the old verdicts and is checked again after the debounce.
async function freshChecks() {
  const f = check;
  const row = (id) => text(`.row[data-id="${id}"]`);
  const repo = join(fixture, 'repos/alpha');
  await click('.row[data-id="k2"]');
  const panel = await text('.panel');
  f('a picked task with a verdict shows the reason, the proof file, «Убрать» and «Оставить»', /⊘ сделано/.test(panel) && /тема уже есть/.test(panel) && /Доказательство в коде:\s*export\.txt/.test(panel) && (await evalJs('!!document.querySelector("[data-c=fresh-remove]") && !!document.querySelector("[data-c=fresh-keep]")')), panel.replace(/\s+/g, ' '));
  await shot('card-fresh.png');
  await click('[data-c=fresh-keep]');
  await waitFor('!/⊘/.test(document.querySelector(".row[data-id=k2]")?.innerText || "")', 4000);
  f('«Оставить» hides the badge (and the buttons)', !/⊘/.test(await row('k2')) && !(await evalJs('!!document.querySelector("[data-c=fresh-remove]")')), await row('k2'));
  f('…but k1 keeps its own', /◐ частично/.test(await row('k1')));
  await key('Escape');
  f('esc drops the pick, the card stays', !(await evalJs('!!document.querySelector(".panel")')) && !!(await evalJs('!!document.querySelector(".tabs")')));

  // A new commit in main: the old verdicts go at once, the check waits for the debounce (2 s here) …
  gitIn(repo, 'commit', '--allow-empty', '-qm', 'Новый коммит в main');
  await key('r');
  const hidden = await waitFor('!/[◐⊘]/.test(document.querySelector(".row[data-id=k1]")?.innerText || "") && !/[◐⊘]/.test(document.querySelector(".row[data-id=k2]")?.innerText || "")', 5000);
  f('a new commit in main hides the verdicts of the old one', hidden, `${await row('k1')} | ${await row('k2')}`);
  f('…and does not ask at once (the debounce)', logLines('fresh.jsonl').length === 1, String(logLines('fresh.jsonl').length));
  await sleep(2600);
  await key('r');
  const again = await waitFor('/⊘ сделано/.test(document.querySelector(".row[data-id=k2]")?.innerText || "")', 8000);
  const fl = logLines('fresh.jsonl');
  f('after the debounce the backlog is checked again, with what changed since the last check', again && fl.length === 2 && /С прошлой проверки \([0-9a-f]+\):/.test(fl[1].prompt) && /Новый коммит в main/.test(fl[1].prompt), (fl[1]?.prompt || '').slice(0, 300));
  f('the badge for the kept task is back at the new commit; k1 is no worry now (no badge)', again && !/[◐⊘]/.test(await row('k1')), `${await row('k1')} | ${await row('k2')}`);

  // «Убрать» asks the coordinator, with the verdict; nothing is removed here.
  await click('.row[data-id="k2"]');
  await click('[data-c=fresh-remove]');
  f('«Убрать» asks «Убрать из бэклога?»', /Убрать из бэклога\?/.test(await text('.panel')), await text('.panel'));
  await key('y');
  const sent = await waitFor('/Просьба убрать задачу отправлена/.test(document.querySelector(".note")?.innerText || "")', 8000);
  const rp = logLines('prompts.jsonl').at(-1);
  f('the request to the coordinator carries the verdict and its reason', sent && /отказался от задачи «Добавить тёмную тему»: удали её из TASKS\.md\. Проверка актуальности отметила её как «⊘ сделано»: тема уже есть\./.test(rp?.text || ''), rp?.text);

  // An agent that fails: a quiet line, not a verdict; the next try waits.
  writeFileSync(join(fixture, 'data', 'fresh-fails'), '');
  gitIn(repo, 'commit', '--allow-empty', '-qm', 'Ещё коммит');
  await key('r');
  await sleep(2600);
  await key('r');
  const failed = await waitFor('/Проверка актуальности не удалась: .*fake agent fails/.test(document.querySelector(".note")?.innerText || "")', 8000);
  f('a failed check says so under the progress bar and gives no verdict', failed && !/[◐⊘]/.test(await row('k1')), await note());
  rmSync(join(fixture, 'data', 'fresh-fails'));
  await shot('card-fresh-error.png');
  f('the failed check asked once, and TASKS.md is as it was', logLines('fresh.jsonl').length === 3 && readFileSync(tasksMd(), 'utf8') === tasksAtStart, String(logLines('fresh.jsonl').length));
}

// ----- the settings screen and the group suggestion (fixture)
async function settingsChecks() {
  const f = check;
  const defs = (sel) => evalJs(`document.querySelectorAll(${JSON.stringify(sel)}).length`);
  const groupRows = () => evalJs('[...document.querySelectorAll(".grow-row")].map((r) => r.querySelector(".gname").textContent.replace(/\\s+/g, " ").trim())');
  await click('[data-act=settings]');
  f('«⚙ Настройки» opens the settings screen', /Настройки/.test(await text('.header .title')) && (await defs('.card[data-row-key^="s:"]')) === 3, await text('.header .title'));
  f('the agent defaults to Claude Haiku 5.5; the checks and suggestions are on / off as seeded', /● Claude Haiku 5\.5/.test(await text('.card[data-row-key="s:agent"] .badge.active')) && /● вкл/.test(await text('.card[data-row-key="s:autocheck"] .badge.active')) && /● выкл/.test(await text('.card[data-row-key="s:autogroup"] .badge.active')));
  const g0 = await groupRows();
  f('the groups card lists the five starting groups with their counts', g0.length === 5 && /^▸?\s*1\. Omarchy\s+0 проектов$/.test(g0[0]) && /5\. Личное/.test(g0[4]), JSON.stringify(g0));
  await shot('settings.png');
  await click('.card[data-row-key="s:agent"] .badge[data-value=deepseek]');
  await waitFor('/DeepSeek/.test(document.querySelector(".card[data-row-key=\'s:agent\'] .badge.active")?.innerText || "")');
  f('choosing the cheap agent sticks (state.json) and shows', (await snapshot()).settings.agent === 'deepseek' && /● DeepSeek Flash/.test(await text('.card[data-row-key="s:agent"] .badge.active')));
  await click('.card[data-row-key="s:agent"] .badge[data-value=haiku]');
  await waitFor('/Haiku/.test(document.querySelector(".card[data-row-key=\'s:agent\'] .badge.active")?.innerText || "")');
  await key('ArrowDown');
  await key('ArrowRight');
  await waitFor('/выкл/.test(document.querySelector(".card[data-row-key=\'s:autocheck\'] .badge.active")?.innerText || "")');
  f('↓ then → turns the selected setting to its next value', (await snapshot()).settings.autocheck === 'off' && /^s:autocheck$/.test(await evalJs('document.querySelector(".card.sel")?.getAttribute("data-row-key")')));
  await key('ArrowLeft');
  await waitFor('/вкл/.test(document.querySelector(".card[data-row-key=\'s:autocheck\'] .badge.active")?.innerText || "")');
  f('← turns it back', (await snapshot()).settings.autocheck === 'on');

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
  f('…it sees the groups with their hints and every project with its goal', /^Группы:\n- Omarchy — проекты вокруг Linux-окружения Omarchy/.test(gl[0].prompt) && /alpha \| Alpha \| Проект для проверки карточки/.test(gl[0].prompt) && /beta \| Beta \| Пустой проект на паузе/.test(gl[0].prompt), gl[0].prompt.slice(0, 200));
  const after = await groupRows();
  f('…and the counts follow: Omarchy 1, Личное 1, ИИ-инфраструктура 1', /Omarchy\s+1 проект$/.test(after[0]) && /ИИ-инфраструктура\s+1 проект$/.test(after[2]) && /Личное\s+1 проект$/.test(after[4]), JSON.stringify(after));
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
  f('group and setting ops: «Прочее» is taken 400, unknown op 400, nowhere to move 400, bad value 400, bad key 400, unknown group 400', [b1, b2, b3, b4, b5, b6].join() === '400,400,400,400,400,400', [b1, b2, b3, b4, b5, b6].join());
  await key('Escape');
  f('esc leaves the settings: the list is back', !!(await evalJs('!!document.querySelector(".controls") && !document.querySelector(".card")')));
  await key('Escape');
}

// ----- the «Новый проект» form (fixture: the real `herdr-projects new` never runs)
async function newProjectChecks() {
  const f = check;
  const val = (id) => evalJs(`document.getElementById(${JSON.stringify(id)})?.value`);
  const newLog = () => logLines('new.jsonl');
  // Turn the suggestion on: nothing waits, so nothing runs.
  await key(',');
  await click('.card[data-row-key="s:autogroup"] .badge[data-value=on]');
  await waitFor('document.querySelector(".card[data-row-key=\'s:autogroup\'] .badge.active")?.innerText.includes("вкл")');
  await sleep(500);
  f('the suggestion on with every project in a group: no call', logLines('groups.jsonl').length === 2, String(logLines('groups.jsonl').length));
  await key('Escape');

  await key('n');
  f('n opens the form with the cursor in «Папка»', /Новый проект/.test(await text('.header .title')) && await evalJs('document.activeElement?.id === "nf-path"'));
  const loaded = await waitFor('/Треды: как в настройках \\(claude-sonnet\\)/.test(document.body.innerText) && /Коорд\\.: как в настройках \\(claude\\)/.test(document.body.innerText)', 4000);
  f('machines and profiles are read when it opens: the profile badges name the defaults', loaded, (await text('.nform')).replace(/\s+/g, ' ').slice(0, 300));
  await shot('new-form.png');
  await click('[data-n=machine][data-i="0"]');
  const m1 = await text('[data-n=machine][data-i="0"]');
  await click('[data-n=machine][data-i="0"]');
  const m2 = await text('[data-n=machine][data-i="0"]');
  f('«Машина» steps through the enabled machines (not the disabled one) and back to «локально»', /@mac/.test(m1) && /@nas/.test(m2), `${m1} | ${m2}`);
  f('a remote folder has no «Обзор»', !(await evalJs('!!document.querySelector("[data-n=browse-toggle]")')));
  await click('[data-n=machine][data-i="0"]');
  f('…and «локально» brings it back', /локально/.test(await text('[data-n=machine][data-i="0"]')) && (await evalJs('!!document.querySelector("[data-n=browse-toggle]")')));
  await key('Escape');
  f('esc cancels: the list is back and nothing was created', !!(await evalJs('!!document.querySelector(".controls")')) && newLog().length === 0);

  await key('n');
  await waitFor('document.activeElement?.id === "nf-path"');
  await typed(`${fixture}/repos/`);
  await key('ArrowDown');
  const open = await waitFor('document.querySelectorAll(".brow").length >= 4', 4000);
  const rows = (await evalJs('[...document.querySelectorAll(".brow")].map((r) => r.innerText.replace(/\\s+/g, " ").trim())')).map((r) => r.replace(/^➜\s*/, ''));
  f('↓ in «Папка» opens the folder browser: .., the folders by name, repositories marked, hidden ones and files not shown', open && JSON.stringify(rows) === JSON.stringify(['..', 'alpha/ ⎇ репозиторий', 'delta/ ⎇ репозиторий', 'plain/']), JSON.stringify(rows));
  await shot('new-browse.png');
  await key('ArrowDown');
  await key('ArrowRight');
  await waitFor('/src\\//.test(document.querySelector(".browse")?.innerText || "")');
  f('→ enters the highlighted folder', /\/repos\/delta/.test(await text('.brow-head')) && /src\//.test(await text('.browse')), await text('.brow-head'));
  await key('ArrowLeft');
  await waitFor('/\\/repos$/.test(document.querySelector(".brow-head")?.innerText.trim() || "")');
  f('← goes up and highlights the folder it came out of', (await evalJs('document.querySelector(".brow.hl")?.innerText.trim()')).includes('delta/'), await text('.brow.hl'));
  await key('Enter');
  f('Enter picks it: the path, the name from the folder, the cursor in «Название»', (await val('nf-path')) === `${fixture}/repos/delta` && (await val('nf-name')) === 'Delta' && (await evalJs('document.activeElement?.id === "nf-name"')) && !(await evalJs('!!document.querySelector(".browse")')), `${await val('nf-path')} | ${await val('nf-name')}`);

  // a second repository, completed with tab, on another machine
  await click('[data-n=repo-add]');
  f('«+ ещё репозиторий» adds «Папка 2» with the cursor in it', await evalJs('document.activeElement?.id === "nf-path2"') && /Папка 2/.test(await text('.nform')));
  await typed(`${fixture}/repos/pla`);
  await key('Tab');
  await waitFor('document.getElementById("nf-path2")?.value.endsWith("/plain/")', 3000);
  f('tab in a folder field completes the path', (await val('nf-path2')) === `${fixture}/repos/plain/`, await val('nf-path2'));
  await click('[data-n=machine][data-i="1"]');
  await evalJs('(() => { const e = document.getElementById("nf-path2"); e.value = "/srv/delta-mac/"; e.dispatchEvent(new Event("input", { bubbles: true })); })()');
  f('a machine for the second repository', /@mac/.test(await text('[data-n=machine][data-i="1"]')));
  await click('[data-n=tprof]'); await click('[data-n=tprof]');
  await click('[data-n=cprof]');
  f('the profile badges step through the allowed ones: threads → haiku, coordinator → claude', /Треды: claude-haiku/.test(await text('[data-n=tprof]')) && /Коорд\.: claude$/.test(await text('[data-n=cprof]')), await text('[data-n=tprof]') + ' | ' + await text('[data-n=cprof]'));
  await evalJs('document.getElementById("nf-goal").focus()');
  await typed('Цель дельты');
  await shot('new-filled.png');
  await key('Enter');
  const created = await waitFor('/Delta/.test(document.querySelector(".header .title")?.innerText || "") && /Проект создан: delta/.test(document.body.innerText)', 8000);
  const nl = newLog();
  f('Enter creates the project: `herdr-projects new` with both repositories, goal and profiles; the name after --', created && nl.length === 1 && JSON.stringify(nl[0].argv) === JSON.stringify([`--repo=${fixture}/repos/delta`, '--repo=/srv/delta-mac@mac', '--goal=Цель дельты', '--thread-profile=claude-haiku', '--coordinator-profile=claude', '--', 'Delta']), JSON.stringify(nl[0]?.argv));
  const opened = await waitFor('/создан и открыт в herdr/.test(document.body.innerText)', 6000);
  f('the new project opens in the page and its coordinator is started (`open`)', opened && /Delta/.test(await text('.header .title')) && logLines('opens.jsonl').some((o) => o.slug === 'delta'), await text('.header .title'));
  const pm = readFileSync(join(fixture, 'root/delta/PROJECT.md'), 'utf8');
  f('the project was made by herdr-projects (the stand-in), not by the page', /name = "Delta"/.test(pm) && /goal = "Цель дельты"/.test(pm) && /machine = "mac"/.test(pm));
  await shot('new-created.png');

  // The suggestion for a new project: one call, only for it; a name the agent coined becomes a group.
  const grouped = await waitFor('(async () => { const s = await fetch("/api/snapshot").then(r => r.json()); return s.groups.assign.delta?.g === "Новая область"; })()', 12000);
  const gl = logLines('groups.jsonl');
  const s = await snapshot();
  f('a new project gets its group guess once: one call that asks only about it', grouped && gl.length === 3 && /\ndelta \| Delta \| Цель дельты/.test(gl[2].prompt) && !/\nalpha \|/.test(gl[2].prompt.split('Проекты')[1]), (gl[2]?.prompt || '').slice(-200));
  f('…a group the agent named is added at the end and the project sits in it as a guess', s.groups.list.at(-1) === 'Новая область' && s.groups.assign.delta?.auto === true, JSON.stringify([s.groups.list, s.groups.assign.delta]));

  // Wrong inputs go back as errors and create nothing.
  const post = (body) => api('POST', '/api/new', body);
  const repo = (path, machine = '') => ({ path, machine });
  const [n1] = await post({ name: 'x', repos: [] });
  const [n2, e2] = await post({ name: 'x', repos: [repo('/x', 'a b')] });
  const [n3, e3] = await post({ name: 'x', repos: [repo(`${fixture}/repos/нет`)] });
  const [n4, e4] = await post({ name: 'Alpha', repos: [repo(`${fixture}/repos/plain`)] });
  const [n5] = await post({ name: 'x', repos: Array.from({ length: 7 }, () => repo('/x', 'm')) });
  const [n6, e6] = await post({ name: 'Zed', repos: [repo(`${fixture}/repos/plain`)], thread_profile: '-bad profile' });
  f('new: no repository 400, a machine with a space 400, a missing folder 400, a taken slug 409, seven repositories 400, a bad profile 400', [n1, n2, n3, n4, n5, n6].join() === '400,400,400,409,400,400' && /без пробелов/.test(e2.error) && /Папка не найдена/.test(e3.error) && /Проект alpha уже есть/.test(e4.error) && /профиля/.test(e6.error), [n1, n2, n3, n4, n5, n6].join());
  writeFileSync(join(fixture, 'data', 'refuse-new'), '');
  const [n7, e7] = await post({ name: 'Epsilon', repos: [repo(`${fixture}/repos/plain`)] });
  rmSync(join(fixture, 'data', 'refuse-new'));
  f('a refusal of herdr-projects comes back as 502 with its words, and no project appears', n7 === 502 && /refusing to create epsilon/.test(e7.error) && !existsSync(join(fixture, 'root/epsilon')), JSON.stringify(e7));
  const [r1] = await api('GET', `/api/browse?path=${encodeURIComponent('/нет/такой')}`);
  const [r2, c2] = await api('GET', `/api/complete?path=${encodeURIComponent(`${fixture}/repos/de`)}`);
  f('browse: an unreadable folder 400; complete: a path is completed', r1 === 400 && r2 === 200 && c2.path === `${fixture}/repos/delta/`, JSON.stringify([r1, r2, c2]));
  f('only the one project was ever made (the second attempt was refused, the others never reached the command)', newLog().length === 2 && logLines('set.jsonl').length === 0, String(newLog().length));
  await key('Escape');
}

// ----- live server: the screens are looked at, nothing is changed or created
async function screensSmoke() {
  await click('[data-act=settings]');
  check('settings: a card per setting and the groups card', (await evalJs('document.querySelectorAll(".card[data-row-key^=\'s:\']").length')) >= 3 && /Группы проектов/.test(await text('.scroll')));
  check('settings: the cheap agent shows Haiku 5.5 as an option', /Claude Haiku 5\.5/.test(await text('.card[data-row-key="s:agent"]')));
  await shot('settings.png');
  await key('Escape');
  check('esc leaves the settings', !!(await evalJs('!!document.querySelector(".controls")')));
  await key('n');
  check('n opens the new-project form', /Новый проект/.test(await text('.header .title')) && !!(await evalJs('!!document.querySelector("#nf-path") && !!document.querySelector("#nf-name") && !!document.querySelector("#nf-goal")')));
  await waitFor('!/Читаю машины и профили/.test(document.body.innerText)', 6000);
  check('the form asks for the machines and profiles (and works without them)', !/Читаю машины и профили/.test(await text('.nform')), (await text('.nform')).replace(/\s+/g, ' ').slice(0, 200));
  await key('ArrowDown');
  const browsed = await waitFor('!!document.querySelector(".browse")', 4000);
  check('↓ in «Папка» opens the folder browser', browsed && (await evalJs('document.querySelectorAll(".brow").length')) > 0);
  await shot('new-form.png');
  await key('Escape');
  check('esc closes the browser, not the form', !(await evalJs('!!document.querySelector(".browse")')) && /Новый проект/.test(await text('.header .title')));
  await key('Escape');
  check('esc leaves the form: nothing was created', !!(await evalJs('!!document.querySelector(".controls")')));
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
  // The bar icon's endpoint agrees with the list it summarises (archived projects do not count).
  const sum = await evalJs(`Promise.all([fetch("/api/summary").then(r => r.json()), fetch("/api/snapshot").then(r => r.json())]).then(([m, s]) => {
    const live = s.projects.filter((p) => p.status !== "archived");
    const add = (f) => live.reduce((n, p) => n + f(p), 0);
    return { m, want: { waiting: add((p) => p.counts.needs), review: add((p) => p.counts.review), inbox: add((p) => p.inbox), projects: live.length } };
  })`);
  check('/api/summary: waiting, review, inbox and projects match the list',
    ['waiting', 'review', 'inbox', 'projects'].every((k) => sum.m[k] === sum.want[k]), JSON.stringify(sum));
  check('/api/summary: small, with the fields the bar icon reads', Object.keys(sum.m).length <= 10
    && ['waiting', 'review', 'updated', 'error'].every((k) => k in sum.m), JSON.stringify(sum.m));
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

  check('the list has «все проекты» next to the sort and group switches', /все проекты\s*U/i.test(await evalJs('document.querySelector("[data-act=unfocus]")?.textContent') || ''));
  if (fixture) {
    // F focuses the highlighted project, U clears the sidebar view; the fake herdr-projects logs both (never the real one).
    await key('ArrowDown');
    if ((await evalJs('document.querySelector(".row.hl")?.getAttribute("data-row")')) === '+new') await key('ArrowUp');
    const hlSlug = await evalJs('document.querySelector(".row.hl[data-row]")?.getAttribute("data-row")');
    await key('f');
    const focused = await waitFor('/Фокус на/.test(document.querySelector(".note")?.innerText || "")');
    check('F focuses the highlighted project', focused && logLines('focus.jsonl').at(-1)?.slug === hlSlug && logLines('unfocus.jsonl').length === 0, `${hlSlug} ${JSON.stringify(logLines('focus.jsonl').at(-1))}`);
    await key('u');
    const cleared = await waitFor('/снова показывает все проекты/.test(document.querySelector(".note")?.innerText || "")');
    check('U runs `herdr-projects unfocus` (no project needed)', cleared && logLines('unfocus.jsonl').length === 1, JSON.stringify(logLines('unfocus.jsonl')));
    await click('[data-act=unfocus]');
    for (let i = 0; i < 25 && logLines('unfocus.jsonl').length < 2; i++) await sleep(100);
    check('the «все проекты» switch does the same by click', logLines('unfocus.jsonl').length === 2);
    await key('?');
    check('? lists the key', /U\s*показать все проекты/i.test(await text('.dock')), await text('.dock'));
    await key('?');
  }
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
  if (fixture) { await settingsChecks(); await newProjectChecks(); } else await screensSmoke();

  ws.close();
}

main().catch((e) => { console.error('ERROR', e); failed++; }).finally(() => {
  chrome.kill();
  rmSync(profile, { recursive: true, force: true });
  process.exit(failed ? 1 : 0);
});
