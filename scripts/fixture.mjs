#!/usr/bin/env node
// A self-contained world for the page checks: the data folder of a herdr-projects
// board (repos, keys, spaces, task records) with a dozen projects whose tasks stand
// in every stage the board knows, and stand-ins for the binaries the planner calls
// (`herdr-projects`, `herdr`, `claude`) that log what they were asked and touch
// no real board, coordinator or sidebar.
//
//   node scripts/fixture.mjs DIR        builds DIR (replacing what is there)
//   HERDR_PROJECTS_BIN=DIR/bin/herdr-projects HERDR_BIN=DIR/bin/herdr HERDR_SOCKET_PATH=DIR/herdr.sock \
//   PATH=DIR/bin:$PATH FIXTURE_DIR=DIR herdr-planner --data DIR/board --port N
//
// The tasks the stand-in `herdr-projects` lists are DIR/state/board.json (it changes
// them when a command is run: approve, retry, …); its logs are `DIR/log/*.jsonl`.

import { chmodSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';

const dir = resolve(process.argv[2] || '');
if (!process.argv[2]) { console.error('usage: node scripts/fixture.mjs DIR'); process.exit(2); }
rmSync(dir, { recursive: true, force: true });
for (const d of ['board', 'bin', 'log', 'state', 'data', 'repos']) mkdirSync(join(dir, d), { recursive: true });

const write = (path, text) => { mkdirSync(join(path, '..'), { recursive: true }); writeFileSync(path, text); };
const minutesAgo = (m) => Date.now() - m * 60000;

// --- the repos of the board and their keys
const repos = {
  alpha: 'ALP', beta: 'BET', gamma: 'GAM', kappa: 'KAP', zeta: 'ZET', iota: 'IOT', eps: 'EPS', rho: 'RHO', theta: 'THE', eta: 'ETA', sigma: 'SIG',
};
const board = join(dir, 'board');
write(join(board, 'repos.json'), JSON.stringify(Object.fromEntries(Object.keys(repos).map((r) => [r, join(dir, 'repos', r)]))));
write(join(board, 'repo-keys.json'), JSON.stringify({ upgraded: true, had: Object.fromEntries(Object.entries(repos).map(([r, k]) => [k, r])) }));
write(join(board, 'spaces.json'), JSON.stringify({ [`/fixture/herdr.sock|${join(dir, 'repos/kappa')}`]: 'wK1' }));

// --- the tasks, as `herdr-projects list --json` prints them; `minutes` is how long ago each came to its stage
const rows = [];
const task = (repo, n, minutes, row) => {
  rows.push({ id: `${repo}/${n}`, app: 'claude', ...row });
  write(join(board, 'tasks', repo, String(n), 'task.json'), JSON.stringify({ id: `${repo}/${n}`, at: minutesAgo(minutes) }));
};
const done = (repo, n, minutes, title, extra = {}) => task(repo, n, minutes, { state: 'done', stage: 'done', title, msg: 'Merged', ...extra });
const passed = { findings: [{ id: 'F1', severity: 'minor', subject: 'Название кнопки можно короче', where: 'ui/button.rs:12', proof: '', preexisting: false }], meets: true, gate: 'major', pass: true };
const failedReview = { findings: [{ id: 'F1', severity: 'critical', subject: 'Кэш не сбрасывается при смене отчёта', where: 'cache.rs:40', proof: 'тест cache_reset падает', preexisting: false }, { id: 'F2', severity: 'major', subject: 'Нет теста на пустой отчёт', where: '', proof: '', preexisting: false }], meets: false, gate: 'major', pass: false };

done('alpha', 1, 180, 'Экспорт отчёта', { pr: 'https://github.com/o/alpha/pull/7', review: passed, reviewRound: 1 });
task('alpha', 2, 6, { state: 'blocked', kind: 'question', stage: 'working', title: 'Схема экспорта', msg: 'Какую схему брать: старую или новую?' });
task('alpha', 3, 20, { state: 'blocked', kind: 'permission', stage: 'plan', title: 'План миграции', msg: 'Plan ready' });
task('alpha', 4, 12, { state: 'blocked', kind: 'permission', stage: 'approval', title: 'Тёмная тема', msg: 'Pass · 1 minor', pr: 'https://github.com/o/alpha/pull/12', review: passed, reviewRound: 1 });
task('alpha', 5, 30, { state: 'blocked', kind: 'permission', stage: 'approval', title: 'Кэш отчётов', msg: 'Review failed · 1 critical, 1 major', review: failedReview, reviewRound: 2 });
task('alpha', 6, 2, { state: 'working', stage: 'working', title: 'Миграция базы', msg: 'Writing migration', progress: 40, eta: 12 });
task('alpha', 7, 4, { state: 'working', stage: 'reviewing', title: 'Документация', msg: 'Under review', progress: 55, reviewRound: 1 });
task('alpha', 8, 50, { state: 'error', stage: 'working', title: 'Сбой сборки', msg: 'The build failed' });
task('alpha', 9, 8, { state: 'working', stage: 'queued', title: 'Очередь за слотом', msg: 'Waiting for a free slot', queue: 1 });
task('alpha', 10, 9, { state: 'working', stage: 'after', title: 'После девятой', msg: 'queued after ALP-9' });
task('alpha', 11, 400, { state: 'idle', stage: 'todo', title: 'Заметка на потом', msg: '' });
task('alpha', 14, 45, { state: 'blocked', kind: 'permission', stage: 'approval', title: 'Ещё один кэш', msg: 'Review failed · 1 critical, 1 major', review: failedReview, reviewRound: 1 });
task('alpha', 12, 60, { state: 'blocked', stage: 'pick', title: 'Три варианта', msg: 'Pick one of 3' });
done('alpha', 13, 7200, 'Старое дело');
done('gamma', 1, 3000, 'Единственное сделанное');
task('gamma', 2, 3000, { state: 'idle', stage: 'todo', title: 'Задача гаммы', msg: '' });
task('kappa', 1, 2, { state: 'blocked', kind: 'question', stage: 'working', title: 'Запускать ли расходы', msg: 'Запускать ли тред по расходам или подождать?' });
task('zeta', 1, 5, { state: 'blocked', kind: 'permission', stage: 'approval', title: 'Готово к вливанию', msg: 'Not reviewed' });
task('iota', 1, 10, { state: 'blocked', stage: 'nosummary', title: 'Остановился без итога', msg: 'The agent stopped without a summary' });
task('eps', 1, 40, { state: 'working', stage: 'reviewing', title: 'Ревью давно', msg: 'Under review', reviewRound: 1 });
task('rho', 1, 8, { state: 'working', stage: 'reviewing', title: 'Ревью недавно', msg: 'Under review', reviewRound: 1 });
task('theta', 1, 1, { state: 'working', stage: 'working', title: 'Идёт работа', msg: 'Writing', progress: 30 });
// Stages the list does not tell apart by their words alone: a stop by hand, a stop at a limit, work whose checks fail, a reviewer that asks.
task('sigma', 1, 100, { state: 'idle', stage: 'stopped', title: 'Остановлена вручную', msg: 'Stopped' });
task('sigma', 2, 100, { state: 'blocked', stage: 'stopped', title: 'Остановлена лимитом', msg: 'Stopped at a limit' });
task('sigma', 3, 101, { state: 'blocked', kind: 'permission', stage: 'approval', title: 'Проверки PR не прошли', msg: "The pull request's checks fail", pr: 'https://github.com/o/sigma/pull/5', review: passed, reviewRound: 1 });
task('sigma', 4, 102, { state: 'blocked', stage: 'reviewneeds', title: 'Ревьюеру нужен ты', msg: 'The reviewer needs you' });
done('eta', 1, 900, 'Сделано раз');
done('eta', 2, 800, 'Сделано два');
// A repo the board lists tasks of although it is not in repos.json (a repo of another machine, or one just removed).
task('orphan', 1, 15, { state: 'idle', stage: 'todo', title: 'Задача без репозитория', msg: '' });
write(join(dir, 'state/board.json'), JSON.stringify({ rows, keys: { ...repos, orphan: 'ORP' } }, null, 1));
write(join(dir, 'state/workspace.json'), JSON.stringify(null));

// --- stand-ins
const bin = (name, body) => { const p = join(dir, 'bin', name); write(p, `#!/usr/bin/env node\n${body}`); chmodSync(p, 0o755); };
const prelude = `const fs = require('node:fs'); const path = require('node:path');
const D = process.env.FIXTURE_DIR; const log = (f, o) => fs.appendFileSync(path.join(D, 'log', f), JSON.stringify(o) + '\\n');
const readStdin = () => { try { return fs.readFileSync(0, 'utf8'); } catch { return ''; } };
const sleep = (ms) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
`;
bin('herdr-projects', `${prelude}
const argv = process.argv.slice(2); const [cmd, ...rest] = argv;
const file = path.join(D, 'state', 'board.json');
const world = () => JSON.parse(fs.readFileSync(file, 'utf8'));
const save = (w) => fs.writeFileSync(file, JSON.stringify(w, null, 1));
const tag = (w, id) => { const [repo, n] = [id.slice(0, id.lastIndexOf('/')), id.slice(id.lastIndexOf('/') + 1)]; return w.keys[repo] ? w.keys[repo] + '-' + n : '#' + n; };
const find = (w, id) => w.rows.find((r) => r.id === id);
if (cmd !== 'list' && cmd !== 'details') log('calls.jsonl', { argv, data: process.env.HERDR_PROJECTS_DATA_DIR || null });
if (cmd === 'list') {
  if (fs.existsSync(path.join(D, 'data', 'list-fails'))) { console.error('herdr-projects: the board is locked'); process.exit(1); }
  if (fs.existsSync(path.join(D, 'data', 'list-garbage'))) { console.log('this is not json'); process.exit(0); }
  const w = world();
  log('lists.jsonl', { argv, data: process.env.HERDR_PROJECTS_DATA_DIR || null });
  if (argv.includes('--json')) console.log(JSON.stringify(w.rows));
  else console.log(w.rows.map((r) => tag(w, r.id).padEnd(9) + ' ' + String(r.state).padEnd(8) + ' [' + r.app + ']  ' + r.title + (r.msg ? '  · ' + r.msg : '')).join('\\n'));
} else if (cmd === 'details') {
  const w = world(); const r = find(w, rest[0]);
  log('details.jsonl', { argv });
  if (!r) { console.error('No task ' + rest[0]); process.exit(1); }
  const n = rest[0].slice(rest[0].lastIndexOf('/') + 1);
  const out = {
    task: { id: r.id, title: r.title, stage: r.stage, msg: r.msg || '', description: 'Описание задачи ' + r.title },
    failure: r.state === 'error' ? { stage: 'agent', title: 'Сборка упала', detail: 'npm test: 3 failed\\n<script>alert(1)</script>', next: 'Повторить: агент получит ошибку.', agentCanFix: true } : null,
    review: r.review || null, reviews: r.review ? Array.from({ length: r.reviewRound || 1 }, () => ({})) : [],
    agent: r.app, handovers: [], research: false, report: null,
    plan: r.stage === 'plan' ? { text: '1. Снять копию базы\\n2. Прогнать миграцию\\n3. Проверить откат' } : null,
    pr: r.pr || null, worktree: '/fixture/worktrees/work-' + n, branch: 'work/' + n,
    limits: { time: { limit: 'none', usedMs: 240000, used: '4m' }, tokens: { limit: 'none', used: 0, total: 0 } },
  };
  console.log(JSON.stringify(out));
} else if (['approve', 'review', 'retry', 'delegate', 'stop', 'tell'].includes(cmd)) {
  const w = world(); const r = find(w, rest[0]);
  if (!r) { console.error('No task ' + rest[0]); process.exit(1); }
  const refuse = path.join(D, 'data', 'refuse');
  if (fs.existsSync(refuse)) { console.error('herdr-projects: cannot ' + cmd + ' ' + rest[0] + ': the merge has a conflict'); process.exit(1); }
  const t = tag(w, r.id);
  if (cmd === 'approve') {
    if (r.stage === 'plan') Object.assign(r, { state: 'working', kind: undefined, stage: 'working', msg: 'Building', progress: 1 });
    else Object.assign(r, { state: 'done', kind: undefined, stage: 'done', msg: 'Merged' });
    console.log(t + ': approved' + (rest.includes('--anyway') ? ' anyway' : ''));
  } else if (cmd === 'review') {
    Object.assign(r, { state: 'working', kind: undefined, stage: 'working', msg: 'Fixing what the review found', progress: 10 });
    console.log(t + ': the review went back to the agent');
  } else if (cmd === 'retry') {
    Object.assign(r, { state: 'working', stage: 'working', msg: 'Trying again', progress: 5 });
    console.log(t + ': the error went back to the agent');
  } else if (cmd === 'delegate') {
    Object.assign(r, { state: 'working', stage: 'working', msg: 'Just started', progress: 0 });
    console.log(t + ': started, an agent is on it');
  } else if (cmd === 'stop') {
    Object.assign(r, { state: 'idle', stage: 'todo', msg: 'Stopped' });
    console.log(t + ': stopped');
  } else {
    console.log(t + ': sent');
  }
  save(w);
} else { console.error('fixture herdr-projects: unsupported ' + argv.join(' ')); process.exit(2); }
`);
bin('herdr', `${prelude}
const argv = process.argv.slice(2);
if (argv[0] === 'workspace' && argv[1] === 'list') {
  const f = JSON.parse(fs.readFileSync(path.join(D, 'state', 'workspace.json'), 'utf8'));
  console.log(JSON.stringify({ result: { workspaces: f ? [{ focused: true, label: f.label + '\\u2800', workspace_id: f.id }] : [{ focused: false, label: 'other', workspace_id: 'w0' }] } }));
} else { console.error('fixture herdr: unsupported ' + argv.join(' ')); process.exit(2); }
`);
bin('claude', `${prelude}
const argv = process.argv.slice(2); const prompt = readStdin();
const entry = { argv, prompt, cwd: process.cwd() };
if (prompt.includes('Проекты (slug | название | о проекте):')) {
  log('groups.jsonl', entry); sleep(300);
  const slugs = prompt.split('Проекты (slug | название | о проекте):')[1].split('\\n').map((l) => l.split(' | ')[0].trim()).filter(Boolean);
  const group = (s) => (s === 'delta' ? 'Новая область' : s.startsWith('a') ? 'Omarchy' : s.startsWith('b') ? 'Личное' : 'ИИ-инфраструктура');
  console.log('Вот раскладка:\\n' + JSON.stringify(slugs.map((slug) => ({ slug, group: group(slug) }))));
} else { log('claude.jsonl', entry); console.log('ok'); }
`);
console.log(dir);
